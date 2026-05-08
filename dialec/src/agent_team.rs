/// Agent Teams orchestration using Claude Code's built-in multi-agent coordination.
///
/// Instead of spawning Claude CLI subprocesses (which hang due to TTY dependency),
/// we use Claude Code Agent Teams:
/// - dialec creates team config and task lists
/// - dialec spawns Claude Code sessions with role-specific prompts
/// - Each session joins the team and reads from the shared task list
/// - dialec polls the filesystem to detect task completion and convergence

use anyhow::{Context, Result, anyhow};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use std::os::unix::process::CommandExt;
use uuid::Uuid;
use chrono::Utc;

use crate::fsutil::{dialec_dir, ensure_dir, write_json_pretty, read_json};
use crate::model::{ConvergenceSignal, Objection, RunTransaction, WorkspaceSnapshot};
use crate::git::snapshot;

/// Agent Team configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTeamConfig {
    pub team_id: String,
    pub team_name: String,
    pub session_id: String,
    pub members: Vec<TeamMember>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamMember {
    pub name: String,
    pub agent_id: String,
    pub agent_type: String,
    pub status: String,
}

/// Create an Agent Team for this dialec session
pub fn create_team(root: &Path, session_id: &str) -> Result<AgentTeamConfig> {
    let team_id = Uuid::new_v4().to_string();
    let team_name = format!("dialec-{}", &team_id[..8]);

    let team_dir = home::home_dir()
        .context("no home directory")?
        .join(".claude")
        .join("teams")
        .join(&team_name);
    ensure_dir(&team_dir)?;

    let task_dir = home::home_dir()
        .context("no home directory")?
        .join(".claude")
        .join("tasks")
        .join(&team_name);
    ensure_dir(&task_dir)?;

    let config = AgentTeamConfig {
        team_id: team_id.clone(),
        team_name: team_name.clone(),
        session_id: session_id.to_string(),
        members: vec![],
        created_at: Utc::now().to_rfc3339(),
    };

    write_json_pretty(&team_dir.join("config.json"), &config)?;

    Ok(config)
}

/// Spawn an agent (claude or codex) as a specific role to create an artifact.
/// Returns the PID of the spawned process so it can be killed when done.
pub fn spawn_agent_for_artifact(
    role: &str,
    phase: &str,
    task_desc: &str,
    workspace: &Path,
    output_file: &Path,
    harness: &str,
    sandbox: &str,
    approval: &str,
) -> Result<u32> {
    let prompt = format!(
        r#"You are the {role} in a Dialec adversarial review phase.

Your task:
{task_desc}

CRITICAL: Write your final output to this exact file:
{output_file}

Your output format MUST be JSON:
{{
  "verdict": "approved" or "rejected",
  "summary": "brief summary of your work",
  "objections": [
    {{
      "type": "correctness|clarity|completeness|architecture|process",
      "severity": "critical|major|minor",
      "issue": "description",
      "location": "where in the spec/code",
      "fix": "how to fix it"
    }}
  ]
}}

Work in: {workspace}

Write to {output_file} and exit when done."#,
        role = role,
        task_desc = task_desc,
        output_file = output_file.display(),
        workspace = workspace.display()
    );

    // Spawn agent session in background
    let mut cmd = Command::new(harness);
    cmd.current_dir(workspace);

    if harness == "codex" {
        // codex: use 'exec' subcommand for non-interactive batch mode
        // Don't use --json (causes stdout streaming, can't write files)
        // Tell codex to write the file directly like Claude does
        let prompt_with_file = format!(
            r#"{}

CRITICAL: Write your final output to this exact file:
{}

Your output format MUST be JSON:
{{
  "verdict": "approved" or "rejected",
  "summary": "brief summary of your work",
  "objections": [
    {{
      "type": "correctness|clarity|completeness|architecture|process",
      "severity": "critical|major|minor",
      "issue": "description",
      "location": "where in the spec/code",
      "fix": "how to fix it"
    }}
  ]
}}

Write to {} and exit when done."#,
            prompt,
            output_file.display(),
            output_file.display()
        );

        cmd.arg("exec")
            .arg("--skip-git-repo-check")
            .arg("-m")
            .arg("gpt-5.5");

        // For codex, approval="never" means disable approvals and sandbox to allow autonomous file writes
        if approval == "never" {
            cmd.arg("--dangerously-bypass-approvals-and-sandbox");
        }

        cmd.arg(&prompt_with_file)
            .stdin(Stdio::null());

        let child = cmd.spawn()
            .with_context(|| format!("failed to spawn {} session", harness))?;

        let pid = child.id();
        Ok(pid)
    } else {
        // claude: use -p flag for prompt (claude writes directly to output_file)
        let prompt_with_file = format!(
            r#"{}

CRITICAL: Write your final output to this exact file:
{}

Write to {} and exit when done."#,
            prompt,
            output_file.display(),
            output_file.display()
        );

        cmd.arg("-p")
            .arg(&prompt_with_file)
            .arg("--dangerously-skip-permissions")
            .stdin(Stdio::null());

        let child = cmd.spawn()
            .with_context(|| format!("failed to spawn {} session", harness))?;

        let pid = child.id();
        Ok(pid)
    }
}

/// Objection as written by agent (codex/claude in adversarial mode)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentObjection {
    #[serde(rename = "type")]
    pub objection_type: String,  // "correctness|clarity|completeness|architecture|process"
    pub severity: String,        // "critical|major|minor"
    pub issue: String,
    pub location: String,
    pub fix: String,
}

/// Result JSON as written by agents during adversarial review
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub verdict: String,
    pub summary: String,
    pub objections: Vec<AgentObjection>,
}

/// Task structure for the shared task list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamTask {
    pub id: String,
    pub subject: String,
    pub description: String,
    pub role: String,
    pub phase: String,
    pub status: String,  // pending, in_progress, completed
    pub owner: Option<String>,
    pub blockers: Vec<String>,
    pub result: Option<TaskResult>,
}

/// Create a task in the team's task list
pub fn create_task(
    team_name: &str,
    role: &str,
    phase: &str,
    task_desc: &str,
) -> Result<String> {
    let task_dir = home::home_dir()
        .context("no home directory")?
        .join(".claude")
        .join("tasks")
        .join(team_name);
    ensure_dir(&task_dir)?;

    let uuid_suffix = Uuid::new_v4().to_string()[..8].to_string();
    let task_id = format!("{}-{}-{}", phase, role, uuid_suffix);

    let task = TeamTask {
        id: task_id.clone(),
        subject: format!("{}: {}", role, phase),
        description: task_desc.to_string(),
        role: role.to_string(),
        phase: phase.to_string(),
        status: "pending".to_string(),
        owner: None,
        blockers: vec![],
        result: None,
    };

    let task_file = task_dir.join(format!("{}.json", task_id));
    write_json_pretty(&task_file, &task)?;

    Ok(task_id)
}

/// Poll for agent output file and parse the result
///
/// Agents write JSON output to a file. This polls for that file's existence
/// and reads the result when it appears.
pub fn poll_agent_output(
    output_file: &Path,
    timeout_secs: u64,
) -> Result<TaskResult> {
    let start = Instant::now();
    let timeout = Duration::from_secs(timeout_secs);
    let mut attempt = 0u32;

    loop {
        attempt += 1;
        eprintln!("[poll_agent_output] Attempt {}: Checking if {} exists", attempt, output_file.display());
        if output_file.exists() {
            eprintln!("[poll_agent_output] File exists, reading...");
            if let Ok(content) = fs::read_to_string(output_file) {
                eprintln!("[poll_agent_output] Read {} bytes", content.len());
                if let Ok(result) = serde_json::from_str::<TaskResult>(&content) {
                    eprintln!("[poll_agent_output] Successfully parsed JSON, verdict={}", result.verdict);
                    return Ok(result);
                } else {
                    eprintln!("[poll_agent_output] Failed to parse JSON from content");
                }
            } else {
                eprintln!("[poll_agent_output] Failed to read file");
            }
        } else {
            eprintln!("[poll_agent_output] File does not exist");
        }

        if start.elapsed() > timeout {
            return Err(anyhow!(
                "Timeout waiting for agent output at {}",
                output_file.display()
            ));
        }

        eprintln!("[poll_agent_output] Sleeping 500ms, elapsed: {:?}", start.elapsed());
        thread::sleep(Duration::from_millis(500));
    }
}

/// Convert task result to RunTransaction for compatibility with existing orchestrator
pub fn task_result_to_transaction(
    root: &Path,
    task_id: &str,
    role: &str,
    phase: &str,
    workspace: &Path,
    result: &TaskResult,
) -> Result<RunTransaction> {
    let now = Utc::now();
    let before = snapshot(workspace);
    let after = snapshot(workspace);

    // Convert AgentObjection to model::Objection for compatibility
    let objections = result.objections.iter().map(|obj| {
        Objection {
            id: Uuid::new_v4().to_string(),
            category: obj.objection_type.clone(),
            severity: obj.severity.clone(),
            description: obj.issue.clone(),
            blocking: matches!(obj.severity.as_str(), "critical" | "major"),
            evidence: obj.issue.clone(),
            proposed_resolution: Some(obj.fix.clone()),
            location: Some(obj.location.clone()),
            owner: None,
            status: "open".to_string(),
        }
    }).collect();

    let signal = ConvergenceSignal {
        verdict: result.verdict.clone(),
        summary: result.summary.clone(),
        objections,
        resolved_objection_ids: vec![],
        new_objection_ids: vec![],
    };

    let turn = RunTransaction {
        id: task_id.to_string(),
        phase: phase.to_string(),
        pod: None,
        role: role.to_string(),
        harness: "claude".to_string(),
        harness_version: Some("agent-team".to_string()),
        workspace: workspace.to_string_lossy().to_string(),
        started_at: now,
        completed_at: now,
        command: crate::model::CommandInvocation {
            argv: vec!["claude".to_string(), "-p".to_string(), "team task".to_string()],
            cwd: workspace.to_string_lossy().to_string(),
            timeout_ms: 1_800_000,
            env_allowlist: std::collections::BTreeMap::new(),
        },
        input_artifacts: vec![],
        before,
        after,
        stdout: crate::model::ArtifactRef {
            id: "stdout".to_string(),
            path: "/dev/null".to_string(),
            sha256: "".to_string(),
            artifact_type: "stdout".to_string(),
        },
        stderr: crate::model::ArtifactRef {
            id: "stderr".to_string(),
            path: "/dev/null".to_string(),
            sha256: "".to_string(),
            artifact_type: "stderr".to_string(),
        },
        event_log: None,
        final_message: crate::model::ArtifactRef {
            id: "final-message".to_string(),
            path: "/dev/null".to_string(),
            sha256: "".to_string(),
            artifact_type: "final-message".to_string(),
        },
        structured: crate::model::ArtifactRef {
            id: "structured".to_string(),
            path: "/dev/null".to_string(),
            sha256: "".to_string(),
            artifact_type: "structured".to_string(),
        },
        signal,
        patch: crate::model::ArtifactRef {
            id: "patch".to_string(),
            path: "/dev/null".to_string(),
            sha256: "".to_string(),
            artifact_type: "patch".to_string(),
        },
        cost: None,
        resume_from: None,
        session_id: None,
        exit_code: 0,
        error: None,
    };

    Ok(turn)
}
