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
/// The agent writes its output to a file, and we poll for that file to detect completion.
pub fn spawn_agent_for_artifact(
    role: &str,
    phase: &str,
    task_desc: &str,
    workspace: &Path,
    output_file: &Path,
    harness: &str,
) -> Result<String> {
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

    let session_id = Uuid::new_v4().to_string();

    // Spawn agent session in background
    let mut cmd = Command::new(harness);
    cmd.current_dir(workspace);

    if harness == "codex" {
        // codex: use 'exec' subcommand for non-interactive batch mode
        cmd.arg("exec")
            .arg("--json")
            .arg("--skip-git-repo-check")
            .arg("--sandbox")
            .arg("read-only")
            .arg("-m")
            .arg("claude")  // Override to use Claude instead of gpt-5-codex
            .arg(&prompt);
    } else {
        // claude: use -p flag for prompt
        cmd.arg("-p")
            .arg(&prompt)
            .arg("--dangerously-skip-permissions");
    }

    cmd.stdin(Stdio::null());

    let _child = cmd.spawn()
        .with_context(|| format!("failed to spawn {} session", harness))?;

    Ok(session_id)
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskResult {
    pub verdict: String,
    pub summary: String,
    pub objections: Vec<Objection>,
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

    loop {
        if output_file.exists() {
            if let Ok(content) = fs::read_to_string(output_file) {
                if let Ok(result) = serde_json::from_str::<TaskResult>(&content) {
                    return Ok(result);
                }
            }
        }

        if start.elapsed() > timeout {
            return Err(anyhow!(
                "Timeout waiting for agent output at {}",
                output_file.display()
            ));
        }

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

    let signal = ConvergenceSignal {
        verdict: result.verdict.clone(),
        summary: result.summary.clone(),
        objections: result.objections.clone(),
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
