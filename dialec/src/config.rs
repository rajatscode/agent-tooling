use crate::fsutil::{dialec_dir, read_json, write_json_pretty};
use crate::model::{
    BudgetConfig, Config, ConvergenceConfig, HarnessConfig, ReminderConfig, Workflow,
    WorkflowPhase, WorkflowStep, WorkspaceConfig,
};
use anyhow::Result;
use serde_json::json;
use std::collections::BTreeMap;
use std::path::Path;

pub fn default_config() -> Config {
    let mut harnesses = BTreeMap::new();
    harnesses.insert(
        "claude".to_string(),
        HarnessConfig {
            command_candidates: vec!["claude".to_string()],
            probe: vec!["--help".to_string(), "--version".to_string()],
            defaults: map([
                ("headless", json!(["-p"])),
                ("outputFormat", json!(["--output-format", "json"])),
                ("systemPrompt", json!(["--append-system-prompt-file"])),
                ("schema", json!(["--json-schema"])),
                ("cwd", json!(null)),
                ("extraFlags", json!([])),
            ]),
        },
    );
    harnesses.insert(
        "codex".to_string(),
        HarnessConfig {
            command_candidates: vec!["codex".to_string()],
            probe: vec![
                "--help".to_string(),
                "--version".to_string(),
                "exec --help".to_string(),
                "debug prompt-input".to_string(),
            ],
            defaults: map([
                ("headless", json!(["exec"])),
                ("outputFormat", json!(["--json"])),
                ("schema", json!(["--output-schema"])),
                ("cwd", json!(["--cd"])),
                ("sandbox", json!(["--sandbox"])),
                ("approval", json!(["--ask-for-approval", "never"])),
                ("finalMessage", json!(["-o"])),
            ]),
        },
    );
    harnesses.insert(
        "gemini".to_string(),
        HarnessConfig {
            command_candidates: vec!["gemini".to_string()],
            probe: vec!["--help".to_string(), "--version".to_string()],
            defaults: map([
                ("headless", json!(["-p"])),
                ("outputFormat", json!(["--output-format", "json"])),
                ("approval", json!(["--approval-mode", "yolo"])),
            ]),
        },
    );
    harnesses.insert(
        "cursor".to_string(),
        HarnessConfig {
            command_candidates: vec!["cursor-agent".to_string()],
            probe: vec!["--help".to_string(), "--version".to_string()],
            defaults: map([
                ("headless", json!(["-p"])),
                ("outputFormat", json!(["--output-format", "json"])),
                ("approval", json!(["--force"])),
            ]),
        },
    );
    harnesses.insert(
        "claudish".to_string(),
        HarnessConfig {
            command_candidates: vec!["claudish".to_string()],
            probe: vec!["--help".to_string(), "--version".to_string()],
            defaults: map([
                ("headless", json!(["-p"])),
                ("outputFormat", json!(["--output-format", "json"])),
                ("schema", json!(["--json-schema"])),
                ("cwd", json!(["--cwd"])),
            ]),
        },
    );

    let roles = map([
        ("coordinator", json!("claude")),
        ("spec-writer", json!("claude")),
        ("spec-reviewer", json!("codex")),
        ("implementer", json!("codex")),
        ("verifier", json!("claude")),
        ("meta-verifier", json!("claude")),
        ("deslopper", json!("claude")),
        ("refactorer", json!("claude")),
        ("adversary", json!("codex")),
        ("arbiter", json!("claude")),
        // Hackathon persistent team
        ("pm", json!("claude")),
        ("arch-lead", json!("codex")),
        ("qa", json!("claude")),
        ("designer", json!("claude")),
        ("researcher", json!("codex")),
    ])
    .into_iter()
    .map(|(k, v)| (k, v.as_str().unwrap().to_string()))
    .collect();

    Config {
        harnesses,
        roles,
        convergence: ConvergenceConfig {
            max_rounds: 5,
            use_arbiter: false,
            arbiter_model: "haiku".to_string(),
            auto_advance_on_nits: true,
            fail_closed_categories: vec![
                "correctness".to_string(),
                "security".to_string(),
                "intent-mismatch".to_string(),
                "test-coverage".to_string(),
                "operability".to_string(),
            ],
        },
        workspaces: WorkspaceConfig {
            strategy: "git-worktree".to_string(),
            base_branch: "current".to_string(),
            keep_failed_workspaces: true,
            dirty_user_workspace_policy: "refuse".to_string(),
        },
        budget: BudgetConfig {
            max_usd: 10.0,
            per_turn_max_usd: 2.0,
            per_phase_max_usd: None,
            max_hours: None,
            max_turns: None,
            deadline: None,
            work_until: None,
        },
        reminders: default_reminders(),
        workflows: default_workflows(),
    }
}

pub fn config_path(root: &Path) -> std::path::PathBuf {
    dialec_dir(root).join("config.json")
}

pub fn load_or_default(root: &Path) -> Result<Config> {
    let path = config_path(root);
    if path.exists() {
        read_json(&path)
    } else {
        Ok(default_config())
    }
}

pub fn write_default_if_missing(root: &Path) -> Result<()> {
    let path = config_path(root);
    if !path.exists() {
        write_json_pretty(&path, &default_config())?;
    }
    Ok(())
}

fn map<const N: usize>(
    items: [(&'static str, serde_json::Value); N],
) -> BTreeMap<String, serde_json::Value> {
    items
        .into_iter()
        .map(|(key, value)| (key.to_string(), value))
        .collect()
}

fn default_reminders() -> ReminderConfig {
    ReminderConfig {
        enabled: true,
        every_turns: 1,
        global_rules: vec![
            "Stay inside your assigned role; do not assume responsibilities assigned to another Dialec role.".to_string(),
            "Preserve auditability: cite artifacts, file paths, commands, and transaction ids when making claims.".to_string(),
            "End every turn with the required structured convergence signal.".to_string(),
            "Do not silently force convergence. Open blockers stay open until resolved, withdrawn, or user-accepted.".to_string(),
        ],
        role_rules: map([
            ("coordinator", json!([
                "Coordinate the phase protocol and dispatch work; do not directly implement source changes unless explicitly acting through the correct role.",
                "Use Dialec commands for worktree management, transactions, status, logs, and phase advancement.",
                "Fail closed on correctness, security, data-loss, migration, operability, and test-coverage blockers."
            ])),
            ("spec-writer", json!([
                "Own the spec artifact and acceptance criteria, not implementation.",
                "Do not edit product source code while acting as spec-writer.",
                "Address every open spec objection explicitly."
            ])),
            ("spec-reviewer", json!([
                "Review the spec adversarially; do not rewrite it in place.",
                "Raise stable, evidence-backed objections for completeness, correctness, clarity, architecture, and intent mismatch.",
                "Approve only when no blocking spec objection remains."
            ])),
            ("implementer", json!([
                "Own code changes for the assigned pod only.",
                "Do not change the frozen spec or erase review artifacts.",
                "Fix valid blockers and defend rejected findings with evidence."
            ])),
            ("verifier", json!([
                "Verify against the frozen spec and run relevant tests/builds when available.",
                "Do not make accepted source patches while acting as verifier; verifier worktree changes are disposable unless Dialec promotes them.",
                "Reject when behavior, tests, security, or operability do not satisfy the spec."
            ])),
            ("meta-verifier", json!([
                "Judge verifier thoroughness, not implementation style.",
                "Do not edit source code.",
                "Send verification back for specific missing checks when needed."
            ])),
            ("deslopper", json!([
                "Review maintainability, local fit, dead code, over-abstraction, and AI slop.",
                "Do not edit source code while acting as deslopper.",
                "Separate blocking maintainability problems from preferences."
            ])),
            ("refactorer", json!([
                "Own cleanup/refactor source changes only after implementation has converged.",
                "Do not remove accepted behavior or introduce backward-compat breaks without user sign-off.",
                "Prefer decomposition and clear intent over abstract cleverness."
            ])),
            ("adversary", json!([
                "Adversarially review cleanup and integration; do not silently fix issues.",
                "Run relevant tests/builds when available.",
                "Reject behavior loss, unapproved compatibility breaks, and weaker decomposition."
            ])),
            ("project-manager", json!([
                "Project managers do not write code.",
                "Clarify scope, decisions, acceptance criteria, and sequencing.",
                "Dispatch implementation/refactor work to the correct implementation role."
            ])),
            ("pm", json!([
                "PMs do not write code.",
                "Own the roadmap and prioritization. Advocate for the user relentlessly.",
                "Propose features with rationale, critique priorities, flag UX issues.",
                "In hackathon mode: propose what to build next, synthesize team input into a final goal.",
                "Produce ranked feature lists with scope estimates when asked to brainstorm."
            ])),
            ("arch-lead", json!([
                "Architecture leads do not write code directly; they review and advise.",
                "Assess technical feasibility of proposals. Flag architectural risks, dependency issues, and scope underestimates.",
                "Recommend implementation approaches, module boundaries, and integration strategies.",
                "Push back on proposals that would create tech debt or architectural inconsistency."
            ])),
            ("qa", json!([
                "QA does not write production code; they identify testing gaps and quality risks.",
                "Review test coverage, flag fragile behavior, suggest hardening work.",
                "Identify edge cases, race conditions, and failure modes the implementation may miss.",
                "Advocate for testing work when the team is biased toward features."
            ])),
            ("designer", json!([
                "Designers do not write implementation code; they advise on UX and API design.",
                "Propose API/interface improvements, flag usability issues and inconsistencies.",
                "Advocate for developer experience: clear naming, intuitive defaults, good error messages.",
                "Review proposals from the perspective of the person who will USE the code."
            ])),
            ("researcher", json!([
                "Researchers investigate, read code, search docs, and gather context. They do not write production code.",
                "For a given proposal, find existing libraries, patterns, prior art, and open issues.",
                "Read relevant source files, TODOs, and git history to inform the team's decisions.",
                "Surface unknowns and blockers before the team commits to a direction."
            ])),
        ])
        .into_iter()
        .map(|(role, value)| {
            (
                role,
                value
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|item| item.as_str().unwrap().to_string())
                    .collect(),
            )
        })
        .collect(),
    }
}

fn default_workflows() -> BTreeMap<String, Workflow> {
    let mut workflows = BTreeMap::new();
    workflows.insert(
        "default".to_string(),
        Workflow {
            description: Some("Built-in spec -> implement -> cleanup phase DAG.".to_string()),
            phases: vec![
                WorkflowPhase {
                    name: "spec".to_string(),
                    depends_on: vec![],
                    steps: vec![
                        WorkflowStep {
                            role: "spec-writer".to_string(),
                            task: "Draft or revise the spec artifact.".to_string(),
                            harness: None,
                            workspace: None,
                            sandbox: "workspace-write".to_string(),
                            approval: "never".to_string(),
                            artifacts: vec![],
                        },
                        WorkflowStep {
                            role: "spec-reviewer".to_string(),
                            task: "Adversarially review the spec artifact.".to_string(),
                            harness: None,
                            workspace: None,
                            sandbox: "read-only".to_string(),
                            approval: "never".to_string(),
                            artifacts: vec![],
                        },
                    ],
                    repeat_until_converged: true,
                    max_rounds: Some(5),
                },
                WorkflowPhase {
                    name: "implement".to_string(),
                    depends_on: vec!["spec".to_string()],
                    steps: vec![],
                    repeat_until_converged: true,
                    max_rounds: Some(5),
                },
                WorkflowPhase {
                    name: "cleanup".to_string(),
                    depends_on: vec!["implement".to_string()],
                    steps: vec![],
                    repeat_until_converged: true,
                    max_rounds: Some(5),
                },
            ],
        },
    );
    workflows
}
