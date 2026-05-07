mod activity;
mod capabilities;
mod channel;
mod config;
mod fsutil;
mod git;
mod ledger;
mod model;
mod orchestrator;
mod schema;
mod session;
mod status;
mod transaction;

use anyhow::{Context, Result};
use capabilities::{probe_all, probe_harness};
use chrono::Utc;
use clap::{Args, Parser, Subcommand, ValueEnum};
use config::load_or_default;
use fsutil::{dialec_dir, project_root};
use ledger::{Ledger, signal_converged};
use model::{CoordinatorState, HarnessCapabilities, RunRequest, Workflow};
use orchestrator::{DriveOptions, drive};
use serde_json::{Value, json};
use session::{
    ensure_layout, log_cost, log_decision, log_timeline, read_state, start_session, write_state,
};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::Duration;
use transaction::run_transaction;

#[derive(Debug, Parser)]
#[command(name = "dialec")]
#[command(about = "Multi-harness orchestrator for structured agent collaboration")]
#[command(version)]
struct Cli {
    #[arg(long, global = true, value_name = "PATH")]
    project: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Init(InitArgs),
    Check(CheckArgs),
    Harnesses(HarnessesArgs),
    Start(StartArgs),
    Drive(DriveArgs),
    Spec(PhaseArgs),
    Implement(PhaseArgs),
    Cleanup(PhaseArgs),
    Tail(TailArgs),
    Cron {
        #[command(subcommand)]
        command: CronCommand,
    },
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommand,
    },
    Status(StatusArgs),
    Log(LogArgs),
    Run(RunArgs),
    Worktree {
        #[command(subcommand)]
        command: WorktreeCommand,
    },
    Finalize(FinalizeArgs),
    Send(SendArgs),
    Inbox(InboxArgs),
    Advance(DecisionArgs),
    Retry(RetryArgs),
    Intervene,
    Release,
    Resume(ResumeArgs),
}

#[derive(Debug, Args)]
struct InitArgs {
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct CheckArgs {
    #[arg(long)]
    no_write: bool,
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct HarnessesArgs {
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct StartArgs {
    #[arg(long, value_enum, default_value_t = Mode::Sidecar)]
    mode: Mode,
    #[arg(long)]
    goal: Option<String>,
    /// Budget: "$10", "4h", "until 8am ET", or combine: "$10, until 8am ET"
    #[arg(long)]
    budget: Option<String>,
    /// Keep working until this time, finding new work when phases complete.
    /// Shorthand for --budget "until <TIME>". Examples: "8am ET", "6:30am", "22:00"
    #[arg(long)]
    until: Option<String>,
    #[arg(long)]
    skip_check: bool,
    #[arg(long)]
    no_drive: bool,
    #[arg(long)]
    drive: bool,
    #[arg(long)]
    foreground: bool,
    #[arg(long, default_value_t = 1)]
    max_parallel: usize,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Mode {
    Sidecar,
    Interactive,
    Autonomous,
}

impl std::fmt::Display for Mode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Mode::Sidecar => write!(f, "sidecar"),
            Mode::Interactive => write!(f, "interactive"),
            Mode::Autonomous => write!(f, "autonomous"),
        }
    }
}

#[derive(Debug, Args)]
struct TailArgs {
    #[arg(long)]
    turn: Option<String>,
    #[arg(long)]
    coordinator: bool,
    #[arg(long)]
    activity: bool,
    #[arg(short, long)]
    follow: bool,
    #[arg(long, default_value = "stdout")]
    stream: String,
    /// Output raw JSONL instead of pretty-printed
    #[arg(long)]
    raw: bool,
}

#[derive(Debug, Subcommand)]
enum CronCommand {
    Tick(CronTickArgs),
    List,
}

#[derive(Debug, Args)]
struct CronTickArgs {
    #[arg(long)]
    role: String,
    #[arg(long, default_value = "spec")]
    phase: String,
    #[arg(long)]
    pod: Option<String>,
}

#[derive(Debug, Subcommand)]
enum WorkflowCommand {
    List,
    Show(WorkflowShowArgs),
    Run(WorkflowRunArgs),
}

#[derive(Debug, Args)]
struct WorkflowShowArgs {
    name: String,
}

#[derive(Debug, Args)]
struct WorkflowRunArgs {
    name: String,
    #[arg(long)]
    max_rounds: Option<u32>,
}

#[derive(Debug, Args)]
struct StatusArgs {
    #[arg(long)]
    json: bool,
    /// Show convergence status for current phase (for agents)
    #[arg(long)]
    convergence: bool,
}

#[derive(Debug, Args)]
struct LogArgs {
    #[arg(long)]
    phase: Option<String>,
    #[arg(long)]
    pod: Option<String>,
    #[arg(long, default_value_t = 50)]
    limit: usize,
}

#[derive(Debug, Args)]
struct DriveArgs {
    #[arg(long)]
    phase: Option<String>,
    #[arg(long)]
    max_rounds: Option<u32>,
    #[arg(long, default_value_t = 1)]
    max_parallel: usize,
    #[arg(long)]
    no_cleanup: bool,
    /// Run harnesses in visible tmux panes
    #[arg(long)]
    pane: bool,
}

#[derive(Debug, Args)]
struct PhaseArgs {
    #[arg(long)]
    max_rounds: Option<u32>,
    #[arg(long, default_value_t = 1)]
    max_parallel: usize,
    /// Run harnesses in visible tmux panes
    #[arg(long)]
    pane: bool,
}

#[derive(Debug, Args)]
struct RunArgs {
    #[arg(long)]
    harness: String,
    #[arg(long)]
    role: String,
    #[arg(long, default_value = "spec")]
    phase: String,
    #[arg(long)]
    task: String,
    #[arg(long)]
    workspace: Option<PathBuf>,
    #[arg(long)]
    pod: Option<String>,
    #[arg(long, default_value = "workspace-write")]
    sandbox: String,
    #[arg(long, default_value = "never")]
    approval: String,
    #[arg(long, default_value_t = 1800)]
    timeout_seconds: u64,
    #[arg(long = "artifact")]
    artifacts: Vec<PathBuf>,
    /// Run the harness in a visible tmux pane instead of capturing silently
    #[arg(long)]
    pane: bool,
}

#[derive(Debug, Subcommand)]
enum WorktreeCommand {
    Create(WorktreeCreateArgs),
    Remove(WorktreeRemoveArgs),
    List,
}

#[derive(Debug, Args)]
struct WorktreeCreateArgs {
    name: String,
    #[arg(long)]
    base: Option<String>,
}

#[derive(Debug, Args)]
struct WorktreeRemoveArgs {
    name: String,
    #[arg(long)]
    delete_branch: bool,
}

#[derive(Debug, Args)]
struct FinalizeArgs {
    /// Turn directory name (e.g. 0001-codex-spec-reviewer)
    #[arg(long)]
    turn: String,
}

#[derive(Debug, Args)]
struct SendArgs {
    /// Target role or turn id to send to
    #[arg(long)]
    to: String,
    /// Message body
    message: String,
    /// Message kind: directive, question, update, cancel, nudge
    #[arg(long, default_value = "directive")]
    kind: String,
    /// Also ping the tmux pane to check inbox
    #[arg(long)]
    ping: bool,
}

#[derive(Debug, Args)]
struct InboxArgs {
    /// Channel to read (role name or turn id)
    target: String,
    /// Only show messages after this id
    #[arg(long)]
    since: Option<String>,
}

#[derive(Debug, Args)]
struct DecisionArgs {
    #[arg(long)]
    reason: String,
    #[arg(long)]
    phase: Option<String>,
    #[arg(long)]
    pod: Option<String>,
}

#[derive(Debug, Args)]
struct RetryArgs {
    #[arg(long)]
    hint: Option<String>,
}

#[derive(Debug, Args)]
struct ResumeArgs {
    session_id: Option<String>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = project_root(cli.project.as_deref())?;

    match cli.command {
        Command::Init(args) => cmd_init(root, args),
        Command::Check(args) => cmd_check(root, args),
        Command::Harnesses(args) => cmd_harnesses(root, args),
        Command::Start(args) => cmd_start(root, args),
        Command::Drive(args) => cmd_drive(root, args),
        Command::Spec(args) => cmd_phase(root, "spec", args),
        Command::Implement(args) => cmd_phase(root, "implement", args),
        Command::Cleanup(args) => cmd_phase(root, "cleanup", args),
        Command::Tail(args) => cmd_tail(root, args),
        Command::Cron { command } => cmd_cron(root, command),
        Command::Workflow { command } => cmd_workflow(root, command),
        Command::Status(args) => cmd_status(root, args),
        Command::Log(args) => cmd_log(root, args),
        Command::Run(args) => cmd_run(root, args),
        Command::Worktree { command } => cmd_worktree(root, command),
        Command::Finalize(args) => cmd_finalize(root, args),
        Command::Send(args) => cmd_send(root, args),
        Command::Inbox(args) => cmd_inbox(root, args),
        Command::Advance(args) => cmd_advance(root, args),
        Command::Retry(args) => cmd_retry(root, args),
        Command::Intervene => cmd_intervene(root),
        Command::Release => cmd_release(root),
        Command::Resume(args) => cmd_resume(root, args),
    }
}

fn cmd_init(root: PathBuf, args: InitArgs) -> Result<()> {
    ensure_layout(&root)?;
    if args.force {
        session::refresh_default_prompts(&root)?;
        println!("refreshed default role prompts and skills; existing config was preserved");
    }
    println!("initialized {}", dialec_dir(&root).display());
    println!("next: dialec check");
    Ok(())
}

fn cmd_check(root: PathBuf, args: CheckArgs) -> Result<()> {
    ensure_layout(&root)?;
    let reports = probe_all(&root, !args.no_write)?;
    let role_errors = validate_role_mappings(&root, &reports)?;

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "harnesses": reports,
                "roleErrors": role_errors,
            }))?
        );
    } else {
        print_harness_table(&reports);
        if role_errors.is_empty() {
            println!("role capability check: ok");
        } else {
            println!("role capability check: failed");
            for error in &role_errors {
                println!("  - {error}");
            }
        }
    }

    if !role_errors.is_empty() {
        anyhow::bail!("role capability check failed");
    }
    Ok(())
}

fn cmd_harnesses(root: PathBuf, args: HarnessesArgs) -> Result<()> {
    ensure_layout(&root)?;
    let reports = probe_all(&root, false)?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        print_harness_table(&reports);
    }
    Ok(())
}

fn cmd_start(root: PathBuf, args: StartArgs) -> Result<()> {
    ensure_layout(&root)?;
    if !args.skip_check {
        let reports = probe_all(&root, true)?;
        let role_errors = validate_role_mappings(&root, &reports)?;
        if !role_errors.is_empty() {
            for error in role_errors {
                eprintln!("role capability error: {error}");
            }
            anyhow::bail!("dialec check failed; rerun with --skip-check only if you know why");
        }
    }
    let should_drive = args.drive && !args.no_drive;
    let should_spawn_coordinator = matches!(args.mode, Mode::Autonomous)
        && args.goal.is_some()
        && !args.no_drive
        && !args.drive;
    // Merge --until into --budget if provided
    let budget = match (args.budget, args.until) {
        (Some(b), Some(u)) => Some(format!("{b}, until {u}")),
        (Some(b), None) => Some(b),
        (None, Some(u)) => Some(format!("until {u}")),
        (None, None) => None,
    };
    let state = start_session(&root, &args.mode.to_string(), args.goal, budget)?;
    println!("started session {}", state.session_id);
    println!("session root: {}", dialec_dir(&root).display());
    if should_drive {
        drive(
            &root,
            DriveOptions {
                max_rounds: None,
                max_parallel: args.max_parallel,
                phase: None,
                no_cleanup: false,
                pane: false,
            },
        )?;
        println!("drive completed");
    } else if should_spawn_coordinator {
        let coordinator = spawn_coordinator(&root, args.foreground)?;
        if args.foreground {
            println!("coordinator completed");
        } else {
            println!("coordinator pid: {}", coordinator.pid);
            println!("tail: dialec tail --coordinator --follow");
        }
    } else {
        match args.mode {
            Mode::Sidecar => {
                println!(
                    "sidecar skill: {}",
                    dialec_dir(&root)
                        .join("skills")
                        .join("sidecar.md")
                        .display()
                );
                println!(
                    "next: use the live Claude session to call dialec run/spec/implement/cleanup, or run `dialec drive` when you want deterministic local autopilot"
                );
            }
            Mode::Interactive => println!("next: dialec run or dialec drive"),
            Mode::Autonomous => println!("next: dialec release or dialec drive"),
        }
    }
    Ok(())
}

fn cmd_drive(root: PathBuf, args: DriveArgs) -> Result<()> {
    ensure_layout(&root)?;
    drive(
        &root,
        DriveOptions {
            max_rounds: args.max_rounds,
            max_parallel: args.max_parallel,
            phase: args.phase,
            no_cleanup: args.no_cleanup,
            pane: args.pane,
        },
    )?;
    println!("drive completed");
    Ok(())
}

fn cmd_phase(root: PathBuf, phase: &str, args: PhaseArgs) -> Result<()> {
    ensure_layout(&root)?;
    drive(
        &root,
        DriveOptions {
            max_rounds: args.max_rounds,
            max_parallel: args.max_parallel,
            phase: Some(phase.to_string()),
            no_cleanup: false,
            pane: args.pane,
        },
    )?;
    println!("{phase} completed");
    Ok(())
}

fn cmd_status(root: PathBuf, args: StatusArgs) -> Result<()> {
    let state = read_state_refreshed(&root).with_context(|| {
        format!(
            "no Dialec session found at {}; run `dialec start`",
            dialec_dir(&root).display()
        )
    })?;

    if args.convergence {
        // Agent query: am I converged? Returns JSON for machine-readable answer.
        let converged = status::is_converged(&root, &state.current_phase)?;
        let blocker_msg = status::blocker_summary(&root, &state.current_phase)?;
        println!("{}", serde_json::to_string_pretty(&json!({
            "phase": state.current_phase,
            "converged": converged,
            "summary": blocker_msg,
        }))?);
        return Ok(());
    }

    if args.json {
        println!("{}", serde_json::to_string_pretty(&state)?);
    } else {
        println!("session: {}", state.session_id);
        println!("mode: {}", state.mode);
        println!("phase: {}", state.current_phase);
        if let Some(goal) = state.goal {
            println!("goal: {goal}");
        }
        println!("started: {}", state.started_at);
        println!("turns: {}", state.total_turns);
        println!(
            "cost: ${:.4} / ${:.4}",
            state.total_cost, state.budget.max_usd
        );
        if let Some(max_hours) = state.budget.max_hours {
            println!("time budget: {max_hours:.2}h");
        }
        if let Some(max_turns) = state.budget.max_turns {
            println!("turn budget: {max_turns}");
        }
        if let Some(coordinator) = state.coordinator {
            println!(
                "coordinator: pid={} status={} stdout={}",
                coordinator.pid, coordinator.status, coordinator.stdout
            );
        }
    }
    Ok(())
}

fn cmd_log(root: PathBuf, args: LogArgs) -> Result<()> {
    let log_path = dialec_dir(&root).join("log").join("timeline.jsonl");
    let content = fs::read_to_string(&log_path)
        .with_context(|| format!("failed to read {}", log_path.display()))?;
    let mut rows: Vec<Value> = content
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| {
            args.phase.as_ref().is_none_or(|phase| {
                value.get("phase").and_then(Value::as_str) == Some(phase.as_str())
            })
        })
        .filter(|value| {
            args.pod
                .as_ref()
                .is_none_or(|pod| value.get("pod").and_then(Value::as_str) == Some(pod.as_str()))
        })
        .collect();
    if rows.len() > args.limit {
        rows = rows.split_off(rows.len() - args.limit);
    }
    for row in rows {
        println!("{}", serde_json::to_string(&row)?);
    }
    Ok(())
}

fn cmd_run(root: PathBuf, args: RunArgs) -> Result<()> {
    ensure_layout(&root)?;
    let workspace = match args.workspace {
        Some(path) => fsutil::normalize(&path)?,
        None => root.clone(),
    };
    let artifacts = args
        .artifacts
        .iter()
        .map(|path| fsutil::normalize(path))
        .collect::<Result<Vec<_>>>()?;
    let req = RunRequest {
        phase: args.phase,
        role: args.role,
        harness: args.harness,
        task: args.task,
        workspace,
        project_root: root.clone(),
        pod: args.pod,
        sandbox: args.sandbox,
        approval: args.approval,
        timeout_ms: args.timeout_seconds * 1000,
        artifacts,
        max_budget_usd: load_or_default(&root)
            .ok()
            .map(|config| config.budget.per_turn_max_usd),
        max_turns: read_state(&root)
            .ok()
            .and_then(|state| state.budget.max_turns),
        pane: args.pane,
    };
    let tx = run_transaction(req)?;
    log_timeline(
        &root,
        json!({
            "event": "turn-completed",
            "transactionId": tx.id,
            "phase": tx.phase,
            "pod": tx.pod,
            "role": tx.role,
            "harness": tx.harness,
            "verdict": tx.signal.verdict,
            "exitCode": tx.exit_code,
            "costUsd": tx.cost.as_ref().and_then(|record| record.usd),
            "at": tx.completed_at
        }),
    )?;
    log_cost(
        &root,
        json!({
            "transactionId": tx.id,
            "phase": tx.phase,
            "pod": tx.pod,
            "role": tx.role,
            "harness": tx.harness,
            "reportedCostUsd": tx.cost.as_ref().and_then(|record| record.usd),
            "cost": tx.cost.clone(),
            "at": tx.completed_at
        }),
    )?;
    println!("turn: {}", tx.id);
    println!("verdict: {}", tx.signal.verdict);
    println!(
        "transaction: {}",
        dialec_dir(&root)
            .join("session")
            .join("turns")
            .join(&tx.id)
            .join("transaction.json")
            .display()
    );
    if let Some(error) = tx.error {
        anyhow::bail!("turn recorded with {} error: {}", error.kind, error.message);
    }
    Ok(())
}

fn cmd_worktree(root: PathBuf, command: WorktreeCommand) -> Result<()> {
    ensure_layout(&root)?;
    match command {
        WorktreeCommand::Create(args) => {
            let path = git::create_worktree(&root, &args.name, args.base.as_deref())?;
            println!("{}", path.display());
        }
        WorktreeCommand::Remove(args) => {
            git::remove_worktree(&root, &args.name, args.delete_branch)?;
            println!("removed {}", args.name);
        }
        WorktreeCommand::List => {
            print!("{}", git::list_worktrees(&root)?);
        }
    }
    Ok(())
}

fn cmd_finalize(root: PathBuf, args: FinalizeArgs) -> Result<()> {
    let tx = transaction::finalize_turn(&root, &args.turn)?;
    // Notify the coordinator channel
    channel::send_message(
        &root,
        &tx.role,
        "coordinator",
        channel::MessageKind::Update,
        &format!(
            "Turn `{}` complete. Role: `{}`, harness: `{}`, verdict: `{}`.\nSummary: {}",
            tx.id, tx.role, tx.harness, tx.signal.verdict, tx.signal.summary
        ),
        Some(json!({
            "turnId": tx.id,
            "phase": tx.phase,
            "pod": tx.pod,
            "verdict": tx.signal.verdict,
            "objectionCount": tx.signal.objections.len(),
            "exitCode": tx.exit_code,
        })),
    )?;
    log_timeline(
        &root,
        json!({
            "event": "turn-finalized",
            "transactionId": tx.id,
            "phase": tx.phase,
            "pod": tx.pod,
            "role": tx.role,
            "harness": tx.harness,
            "verdict": tx.signal.verdict,
            "exitCode": tx.exit_code,
            "at": tx.completed_at,
        }),
    )?;
    println!("finalized {}: verdict={}", tx.id, tx.signal.verdict);
    Ok(())
}

fn cmd_send(root: PathBuf, args: SendArgs) -> Result<()> {
    let kind = match args.kind.as_str() {
        "directive" => channel::MessageKind::Directive,
        "question" => channel::MessageKind::Question,
        "update" => channel::MessageKind::Update,
        "cancel" => channel::MessageKind::Cancel,
        "nudge" => channel::MessageKind::Nudge,
        other => anyhow::bail!("unknown message kind: {other}; use directive/question/update/cancel/nudge"),
    };
    let msg = channel::send_message(&root, "coordinator", &args.to, kind, &args.message, None)?;
    log_timeline(
        &root,
        json!({
            "event": "message-sent",
            "messageId": msg.id,
            "from": msg.from,
            "to": msg.to,
            "kind": args.kind,
            "at": msg.at
        }),
    )?;
    println!("sent {} to {}: {}", args.kind, args.to, msg.id);

    // If --ping and we're in tmux, send a keystroke to the target pane to trigger inbox check
    if args.ping {
        ping_pane(&args.to);
    }
    Ok(())
}

fn cmd_inbox(root: PathBuf, args: InboxArgs) -> Result<()> {
    let messages = channel::read_inbox_since(&root, &args.target, args.since.as_deref())?;
    if messages.is_empty() {
        println!("no messages for {}", args.target);
        return Ok(());
    }
    for msg in &messages {
        println!(
            "[{}] {} → {}: {}",
            msg.kind, msg.from, msg.to, msg.body
        );
    }
    Ok(())
}

/// Ping a tmux pane to remind the agent to check its inbox.
/// This writes a visible reminder to the pane's stdin if possible.
fn ping_pane(target: &str) {
    // Find panes with the target name in their title
    let pane_id = find_pane_by_title(target);
    if let Some(pane) = pane_id {
        // Send a newline + visible message via tmux
        let msg = format!(
            "echo '\\n\\033[1;33m[dialec] New message in inbox. Run: cat .dialec/channels/{}/inbox.jsonl | tail -1 | jq .\\033[0m'",
            target
        );
        let _ = std::process::Command::new("tmux")
            .args(["send-keys", "-t", &pane, &msg, "Enter"])
            .status();
    }
}

fn find_pane_by_title(target: &str) -> Option<String> {
    let output = std::process::Command::new("tmux")
        .args(["list-panes", "-a", "-F", "#{pane_id}:#{pane_title}"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        if let Some((id, title)) = line.split_once(':') {
            if title.contains(target) {
                return Some(id.to_string());
            }
        }
    }
    None
}

fn cmd_advance(root: PathBuf, args: DecisionArgs) -> Result<()> {
    let mut state = read_state(&root)?;
    let phase = args.phase.unwrap_or_else(|| state.current_phase.clone());
    let pod = args.pod.clone();
    let reason = args.reason.clone();
    let blockers = Ledger::read(&root)?.open_blocking(&phase, args.pod.as_deref());
    let accepted: Vec<_> = blockers.iter().map(|entry| entry.id.clone()).collect();
    for id in &accepted {
        session::append_objections(
            &root,
            &[json!({
                "event": "resolved",
                "id": id,
                "phase": phase,
                "pod": pod.clone(),
                "status": "user-accepted",
                "reason": reason.clone(),
                "at": Utc::now()
            })],
        )?;
    }
    state.current_phase = match phase.as_str() {
        "spec" => "implement".to_string(),
        "implement" => "cleanup".to_string(),
        "cleanup" => "done".to_string(),
        other => other.to_string(),
    };
    write_state(&root, &state)?;
    log_decision(
        &root,
        json!({
            "event": "force-advance",
            "reason": reason.clone(),
            "phase": phase,
            "pod": pod.clone(),
            "acceptedObjections": accepted,
            "nextPhase": state.current_phase,
            "at": Utc::now()
        }),
    )?;
    println!("recorded force advance decision");
    Ok(())
}

fn cmd_retry(root: PathBuf, args: RetryArgs) -> Result<()> {
    log_decision(
        &root,
        json!({
            "event": "force-retry",
            "hint": args.hint,
            "at": Utc::now()
        }),
    )?;
    println!("recorded retry decision");
    Ok(())
}

fn cmd_intervene(root: PathBuf) -> Result<()> {
    let mut state = read_state(&root)?;
    // Write stop file so watchdog doesn't restart
    let stop_file = dialec_dir(&root).join("coordinator.stop");
    fs::write(&stop_file, format!("intervened at {}\n", Utc::now()))?;
    if let Some(mut coordinator) = state.coordinator.clone()
        && coordinator.status == "running"
    {
        // Kill the entire process group (watchdog + all children including Claude)
        let _ = ProcessCommand::new("kill")
            .args(["--", &format!("-{}", coordinator.pid)])
            .status();
        // Also kill by PID directly as fallback
        let _ = ProcessCommand::new("kill")
            .arg(coordinator.pid.to_string())
            .status();
        // Find and kill any orphaned Claude processes for this session
        let _ = ProcessCommand::new("pkill")
            .args(["-f", &format!("dialec.*{}", state.session_id)])
            .status();
        let dead = ProcessCommand::new("kill")
            .args(["-0", &coordinator.pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| !s.success())  // kill -0 fails = process is dead = good
            .unwrap_or(true);
        coordinator.status = if dead {
            "intervened".to_string()
        } else {
            "kill-failed".to_string()
        };
        state.coordinator = Some(coordinator);
    }
    state.mode = "sidecar".to_string();
    write_state(&root, &state)?;
    log_timeline(
        &root,
        json!({
            "event": "intervened",
            "mode": "sidecar",
            "phase": state.current_phase,
            "at": Utc::now()
        }),
    )?;
    println!("mode: sidecar");
    println!(
        "sidecar skill: {}",
        dialec_dir(&root)
            .join("skills")
            .join("sidecar.md")
            .display()
    );
    Ok(())
}

fn cmd_release(root: PathBuf) -> Result<()> {
    let mut state = read_state(&root)?;
    state.mode = "autonomous".to_string();
    write_state(&root, &state)?;
    let coordinator = spawn_coordinator(&root, false)?;
    log_timeline(
        &root,
        json!({
            "event": "released",
            "mode": "autonomous",
            "phase": read_state(&root)?.current_phase,
            "pid": coordinator.pid,
            "at": Utc::now()
        }),
    )?;
    println!("mode: autonomous");
    println!("coordinator pid: {}", coordinator.pid);
    println!("tail: dialec tail --coordinator --follow");
    Ok(())
}

fn spawn_coordinator(root: &Path, foreground: bool) -> Result<CoordinatorState> {
    ensure_layout(root)?;
    let mut state = read_state(root)?;
    let config = load_or_default(root)?;
    let report = probe_harness("claude", root)?;
    if !report.available {
        anyhow::bail!("cannot spawn coordinator: claude harness is unavailable");
    }
    let program = report.command.unwrap_or_else(|| "claude".to_string());
    let stdout_path = dialec_dir(root)
        .join("log")
        .join(format!("coordinator-{}.stdout.jsonl", state.session_id));
    let stderr_path = dialec_dir(root)
        .join("log")
        .join(format!("coordinator-{}.stderr.log", state.session_id));
    let prompt = coordinator_prompt(root, &state)?;
    let role = dialec_dir(root).join("roles").join("coordinator.md");
    let stop_file = dialec_dir(root).join("coordinator.stop");
    // Remove stale stop file
    let _ = fs::remove_file(&stop_file);

    let mut claude_args = vec![
        "-p".to_string(),
        prompt,
        "--verbose".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--dangerously-skip-permissions".to_string(),
        "--append-system-prompt-file".to_string(),
        role.to_string_lossy().to_string(),
        "--allowedTools".to_string(),
        "Bash,Read,Glob,Grep,Edit,Write".to_string(),
    ];
    if config.budget.max_usd > 0.0 {
        claude_args.push("--max-budget-usd".to_string());
        claude_args.push(format!("{:.4}", config.budget.max_usd));
    }

    // Build a watchdog script that restarts the coordinator on failure
    let watchdog_path = dialec_dir(root).join("coordinator-watchdog.sh");
    let mut watchdog = String::new();
    watchdog.push_str("#!/bin/bash\n");
    watchdog.push_str(&format!("cd '{}'\n", root.display()));
    watchdog.push_str(&format!("STOP_FILE='{}'\n", stop_file.display()));
    watchdog.push_str(&format!("STDOUT='{}'\n", stdout_path.display()));
    watchdog.push_str(&format!("STDERR='{}'\n", stderr_path.display()));
    watchdog.push_str("MAX_RESTARTS=10\n");
    watchdog.push_str("RESTART_DELAY=30\n");
    watchdog.push_str("RESTARTS=0\n\n");
    watchdog.push_str("while true; do\n");
    watchdog.push_str("  # Check stop conditions\n");
    watchdog.push_str("  if [ -f \"$STOP_FILE\" ]; then\n");
    watchdog.push_str("    echo \"[watchdog] stop file found, exiting\" >> \"$STDERR\"\n");
    watchdog.push_str("    exit 0\n");
    watchdog.push_str("  fi\n\n");
    // Check deadline
    if let Some(deadline) = config.budget.deadline {
        watchdog.push_str(&format!(
            "  if [ $(date -u +%s) -ge {} ]; then\n",
            deadline.timestamp()
        ));
        watchdog.push_str("    echo \"[watchdog] deadline reached, exiting\" >> \"$STDERR\"\n");
        watchdog.push_str("    exit 0\n");
        watchdog.push_str("  fi\n\n");
    }
    watchdog.push_str("  # Run coordinator\n");
    watchdog.push_str(&format!("  '{}' \\\n", program));
    for arg in &claude_args {
        let escaped = arg.replace('\'', "'\\''");
        watchdog.push_str(&format!("    '{}' \\\n", escaped));
    }
    watchdog.push_str("    >> \"$STDOUT\" 2>> \"$STDERR\"\n");
    watchdog.push_str("  EC=$?\n\n");
    watchdog.push_str("  # Exit 0 = clean completion, don't restart\n");
    watchdog.push_str("  if [ $EC -eq 0 ]; then\n");
    watchdog.push_str("    echo \"[watchdog] coordinator exited cleanly\" >> \"$STDERR\"\n");
    watchdog.push_str("    exit 0\n");
    watchdog.push_str("  fi\n\n");
    watchdog.push_str("  # Check stop file again after exit\n");
    watchdog.push_str("  if [ -f \"$STOP_FILE\" ]; then\n");
    watchdog.push_str("    echo \"[watchdog] stop file found after crash, exiting\" >> \"$STDERR\"\n");
    watchdog.push_str("    exit 0\n");
    watchdog.push_str("  fi\n\n");
    watchdog.push_str("  RESTARTS=$((RESTARTS + 1))\n");
    watchdog.push_str("  if [ $RESTARTS -ge $MAX_RESTARTS ]; then\n");
    watchdog.push_str("    echo \"[watchdog] max restarts ($MAX_RESTARTS) reached, giving up\" >> \"$STDERR\"\n");
    watchdog.push_str("    exit 1\n");
    watchdog.push_str("  fi\n\n");
    watchdog.push_str(&format!(
        "  echo \"[watchdog] coordinator exited with $EC, restarting in ${{RESTART_DELAY}}s (attempt $RESTARTS/$MAX_RESTARTS)\" >> \"$STDERR\"\n"
    ));
    watchdog.push_str("  sleep $RESTART_DELAY\n");
    watchdog.push_str("done\n");
    fs::write(&watchdog_path, &watchdog)?;

    ProcessCommand::new("chmod")
        .args(["+x", &watchdog_path.to_string_lossy()])
        .status()?;

    let mut child = ProcessCommand::new("bash")
        .arg(&watchdog_path)
        .current_dir(root)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| "failed to spawn coordinator watchdog")?;

    let pid = child.id();
    let mut command = vec![program];
    command.extend(claude_args);
    let mut coordinator = CoordinatorState {
        pid,
        started_at: Utc::now(),
        stdout: fsutil::relative_to(&stdout_path, root),
        stderr: fsutil::relative_to(&stderr_path, root),
        command,
        status: "running".to_string(),
    };
    state.mode = "autonomous".to_string();
    state.coordinator = Some(coordinator.clone());
    write_state(root, &state)?;
    log_timeline(
        root,
        json!({
            "event": "coordinator-started",
            "pid": pid,
            "watchdog": true,
            "maxRestarts": 10,
            "restartDelay": 30,
            "stdout": coordinator.stdout,
            "stderr": coordinator.stderr,
            "foreground": foreground,
            "at": Utc::now()
        }),
    )?;
    if foreground {
        let status = child
            .wait()
            .with_context(|| format!("failed to wait for coordinator watchdog pid {pid}"))?;
        coordinator.status = if status.success() {
            "completed".to_string()
        } else {
            format!("failed:{status}")
        };
        let mut state = read_state(root)?;
        state.coordinator = Some(coordinator.clone());
        write_state(root, &state)?;
        log_timeline(
            root,
            json!({
                "event": "coordinator-completed",
                "pid": pid,
                "status": coordinator.status,
                "at": Utc::now()
            }),
        )?;
    }
    Ok(coordinator)
}

fn coordinator_prompt(root: &Path, state: &model::DialecState) -> Result<String> {
    let goal = state
        .goal
        .clone()
        .unwrap_or_else(|| "Continue the current Dialec session.".to_string());
    Ok(format!(
        "# Dialec Autonomous Session\n\nProject root: `{}`\nSession id: `{}`\nCurrent phase: `{}`\nGoal:\n{}\n\nStart by running `dialec status`. Drive the session to completion using Dialec commands. Prefer `dialec drive` for deterministic audited autopilot when it fits; otherwise use `dialec run`, `dialec worktree`, `dialec cron tick`, `dialec log`, `dialec advance`, and `dialec retry` according to the coordinator role prompt. Write `.dialec/session/final-report.md` when done.\n",
        root.display(),
        state.session_id,
        state.current_phase,
        goal
    ))
}

fn cmd_tail(root: PathBuf, args: TailArgs) -> Result<()> {
    let path = if args.activity {
        dialec_dir(&root).join("log").join("activity.jsonl")
    } else if args.coordinator {
        let state = read_state(&root)?;
        let coordinator = state
            .coordinator
            .context("no coordinator recorded in session state")?;
        let rel = if args.stream == "stderr" {
            coordinator.stderr
        } else {
            coordinator.stdout
        };
        root.join(rel)
    } else if let Some(turn) = args.turn {
        let file = match args.stream.as_str() {
            "stderr" => "stderr.log",
            "events" => "events.jsonl",
            "final" => "final-message.md",
            "transaction" => "transaction.json",
            _ => "stdout.log",
        };
        dialec_dir(&root)
            .join("session")
            .join("turns")
            .join(turn)
            .join(file)
    } else {
        anyhow::bail!("provide --activity, --coordinator, or --turn <id>");
    };
    tail_file(&path, args.follow, args.raw)
}

fn tail_file(path: &Path, follow: bool, raw: bool) -> Result<()> {
    let mut file =
        fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut content = String::new();
    file.read_to_string(&mut content)?;
    if raw {
        print!("{content}");
    } else {
        for line in content.lines() {
            pretty_print_event(line);
        }
    }
    if !follow {
        return Ok(());
    }
    let mut pos = file.stream_position()?;
    let mut no_change_count = 0u32;
    loop {
        thread::sleep(Duration::from_millis(500));
        let len = fs::metadata(path)?.len();
        if len < pos {
            pos = 0;
        }
        if len == pos {
            // File hasn't grown. Increment counter.
            no_change_count += 1;
            // If file hasn't changed for 5 seconds (10 iterations × 500ms), stop following.
            // This handles the case where the process writing to the file has finished.
            if no_change_count >= 10 {
                break;
            }
            continue;
        }
        // File has new data, reset counter
        no_change_count = 0;
        file.seek(SeekFrom::Start(pos))?;
        let mut buf = String::new();
        file.read_to_string(&mut buf)?;
        if raw {
            print!("{buf}");
        } else {
            for line in buf.lines() {
                pretty_print_event(line);
            }
        }
        pos = file.stream_position()?;
    }
    Ok(())
}

fn pretty_print_event(line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        println!("{line}");
        return;
    };

    // Activity log events use "event" field, coordinator uses "type"
    let event_type = v.get("event")
        .or_else(|| v.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("?");

    let at = v.get("at").and_then(Value::as_str).unwrap_or("");

    // Format timestamp short
    let time_short = if let Some(ts) = at.split('T').nth(1) {
        ts.split('+').next().unwrap_or("?")
    } else {
        "?"
    };

    match event_type {
        "turn-start" => {
            let turn_id = v.get("turn_id").and_then(Value::as_str).unwrap_or("?");
            let phase = v.get("phase").and_then(Value::as_str).unwrap_or("?");
            let role = v.get("role").and_then(Value::as_str).unwrap_or("?");
            let harness = v.get("harness").and_then(Value::as_str).unwrap_or("?");
            println!("\x1b[1;36m[{time_short}]\x1b[0m \x1b[1;37mTURN START\x1b[0m {turn_id} phase={phase} role={role} harness={harness}");
        }
        "probe-harness" => {
            let harness = v.get("harness").and_then(Value::as_str).unwrap_or("?");
            let available = v.get("available").and_then(Value::as_bool).unwrap_or(false);
            let status = if available { "\x1b[1;32m✓\x1b[0m" } else { "\x1b[1;31m✗\x1b[0m" };
            println!("\x1b[1;36m[{time_short}]\x1b[0m {status} probe harness={harness}");
        }
        "workspace-snapshot" => {
            let branch = v.get("before")
                .and_then(|b| b.get("branch"))
                .and_then(Value::as_str)
                .unwrap_or("?");
            let dirty = v.get("before")
                .and_then(|b| b.get("dirty"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let dirty_marker = if dirty { " \x1b[1;33m[dirty]\x1b[0m" } else { "" };
            println!("\x1b[1;36m[{time_short}]\x1b[0m workspace snapshot branch={branch}{dirty_marker}");
        }
        "agent-output" => {
            let turn_id = v.get("turn_id").and_then(Value::as_str).unwrap_or("?");
            let agent = v.get("agent").and_then(Value::as_str).unwrap_or("?");
            println!("\x1b[1;36m[{time_short}]\x1b[0m \x1b[0;35m[output]\x1b[0m {turn_id} from {agent}");
        }
        "agent-complete" => {
            let turn_id = v.get("turn_id").and_then(Value::as_str).unwrap_or("?");
            let exit_code = v.get("exit_code").and_then(Value::as_i64).unwrap_or(-1);
            let status = if exit_code == 0 { "\x1b[1;32m✓\x1b[0m" } else { "\x1b[1;31m✗\x1b[0m" };
            println!("\x1b[1;36m[{time_short}]\x1b[0m {status} agent complete {turn_id} exit_code={exit_code}");
        }
        "signal-parsed" => {
            let turn_id = v.get("turn_id").and_then(Value::as_str).unwrap_or("?");
            let verdict = v.get("verdict").and_then(Value::as_str).unwrap_or("?");
            let color = match verdict {
                "approve" => "\x1b[1;32m",
                "reject" => "\x1b[1;31m",
                _ => "\x1b[1;33m",
            };
            println!("\x1b[1;36m[{time_short}]\x1b[0m {color}SIGNAL\x1b[0m {turn_id} verdict={verdict}");
        }
        "convergence-check" => {
            let phase = v.get("phase").and_then(Value::as_str).unwrap_or("?");
            let converged = v.get("converged").and_then(Value::as_bool).unwrap_or(false);
            let blockers = v.get("blockers").and_then(Value::as_i64).unwrap_or(0);
            let status = if converged {
                "\x1b[1;32m✓ CONVERGED\x1b[0m".to_string()
            } else {
                format!("\x1b[1;31m✗ BLOCKED\x1b[0m ({blockers} blockers)")
            };
            println!("\x1b[1;36m[{time_short}]\x1b[0m {status} phase={phase}");
        }
        "phase-transition" => {
            let from = v.get("from").and_then(Value::as_str).unwrap_or("?");
            let to = v.get("to").and_then(Value::as_str).unwrap_or("?");
            println!("\x1b[1;36m[{time_short}]\x1b[0m \x1b[1;36m→ PHASE\x1b[0m {from} → {to}");
        }
        "system" => {
            let model = v.get("model").and_then(Value::as_str).unwrap_or("?");
            let session = v.get("session_id").and_then(Value::as_str).unwrap_or("?");
            let session_short = &session[..session.len().min(12)];
            println!("\x1b[1;36m[init]\x1b[0m model=\x1b[1m{model}\x1b[0m session={session_short}...");
        }
        "assistant" => {
            let content = v.get("message")
                .and_then(|m| m.get("content"))
                .and_then(Value::as_array);
            if let Some(blocks) = content {
                for block in blocks {
                    let block_type = block.get("type").and_then(Value::as_str).unwrap_or("?");
                    match block_type {
                        "thinking" => {
                            let text = block.get("thinking").and_then(Value::as_str).unwrap_or("");
                            let preview = &text[..text.len().min(200)];
                            println!("\x1b[0;35m[thinking]\x1b[0m {preview}{}",
                                if text.len() > 200 { "..." } else { "" });
                        }
                        "text" => {
                            let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                            if !text.is_empty() {
                                println!("\x1b[1;37m[output]\x1b[0m {text}");
                            }
                        }
                        "tool_use" => {
                            let name = block.get("name").and_then(Value::as_str).unwrap_or("?");
                            let input = block.get("input").and_then(|i| {
                                if let Some(cmd) = i.get("command").and_then(Value::as_str) {
                                    Some(cmd.to_string())
                                } else if let Some(pattern) = i.get("pattern").and_then(Value::as_str) {
                                    Some(pattern.to_string())
                                } else if let Some(path) = i.get("file_path").and_then(Value::as_str) {
                                    Some(path.to_string())
                                } else {
                                    serde_json::to_string(i).ok()
                                        .map(|s| if s.len() > 120 { format!("{}...", &s[..120]) } else { s })
                                }
                            }).unwrap_or_default();
                            println!("\x1b[1;33m[tool]\x1b[0m \x1b[1m{name}\x1b[0m {input}");
                        }
                        "tool_result" => {
                            let content_text = block.get("content").and_then(Value::as_str).unwrap_or("");
                            let preview = &content_text[..content_text.len().min(200)];
                            if !preview.is_empty() {
                                println!("\x1b[0;33m[result]\x1b[0m {preview}{}",
                                    if content_text.len() > 200 { "..." } else { "" });
                            }
                        }
                        _ => {}
                    }
                }
            }
            // Check for errors
            if let Some(err) = v.get("error").and_then(Value::as_str) {
                println!("\x1b[1;31m[error]\x1b[0m {err}");
            }
        }
        "result" => {
            let result = v.get("result").and_then(Value::as_str).unwrap_or("?");
            let cost = v.get("total_cost_usd").and_then(Value::as_f64).unwrap_or(0.0);
            let turns = v.get("num_turns").and_then(Value::as_u64).unwrap_or(0);
            let duration = v.get("duration_ms").and_then(Value::as_u64).unwrap_or(0);
            let preview = &result[..result.len().min(300)];
            println!("\x1b[1;32m[done]\x1b[0m {preview}{}",
                if result.len() > 300 { "..." } else { "" });
            println!("\x1b[0;32m       cost=${cost:.4} turns={turns} duration={duration}ms\x1b[0m");
        }
        "rate_limit_event" => {
            let limit_type = v.get("rate_limit_info")
                .and_then(|r| r.get("rateLimitType"))
                .and_then(Value::as_str)
                .unwrap_or("?");
            let resets = v.get("rate_limit_info")
                .and_then(|r| r.get("resetsAt"))
                .and_then(Value::as_u64)
                .unwrap_or(0);
            println!("\x1b[1;31m[rate-limit]\x1b[0m type={limit_type} resets_at={resets}");
        }
        _ => {
            // Unknown event type — print compact
            let compact = serde_json::to_string(&v).unwrap_or_else(|_| line.to_string());
            let preview = &compact[..compact.len().min(150)];
            println!("\x1b[0;90m[{event_type}]\x1b[0m {preview}{}",
                if compact.len() > 150 { "..." } else { "" });
        }
    }
}

fn cmd_cron(root: PathBuf, command: CronCommand) -> Result<()> {
    ensure_layout(&root)?;
    match command {
        CronCommand::Tick(args) => {
            let text = session::reminder_text(&root, &args.role, &args.phase, args.pod.as_deref())?
                .context("reminders are disabled")?;
            let path = dialec_dir(&root)
                .join("session")
                .join("reminders")
                .join(format!(
                    "{}-{}-{}.md",
                    Utc::now().format("%Y%m%dT%H%M%SZ"),
                    session::sanitize(&args.phase),
                    session::sanitize(&args.role)
                ));
            fs::write(&path, &text)?;
            log_timeline(
                &root,
                json!({
                    "event": "role-reminder-cron",
                    "phase": args.phase,
                    "pod": args.pod,
                    "role": args.role,
                    "path": path,
                    "at": Utc::now()
                }),
            )?;
            print!("{text}");
        }
        CronCommand::List => {
            let config = load_or_default(&root)?;
            println!("enabled: {}", config.reminders.enabled);
            println!("everyTurn: {}", config.reminders.every_turns);
            for rule in config.reminders.global_rules {
                println!("global: {rule}");
            }
            for (role, rules) in config.reminders.role_rules {
                for rule in rules {
                    println!("{role}: {rule}");
                }
            }
        }
    }
    Ok(())
}

fn cmd_workflow(root: PathBuf, command: WorkflowCommand) -> Result<()> {
    ensure_layout(&root)?;
    match command {
        WorkflowCommand::List => {
            let config = load_or_default(&root)?;
            for name in config.workflows.keys() {
                println!("{name}");
            }
            let workflows_dir = dialec_dir(&root).join("workflows");
            if workflows_dir.exists() {
                for entry in fs::read_dir(workflows_dir)? {
                    let entry = entry?;
                    if entry.path().extension().and_then(|ext| ext.to_str()) == Some("json")
                        && let Some(stem) = entry.path().file_stem().and_then(|stem| stem.to_str())
                    {
                        println!("{stem}");
                    }
                }
            }
        }
        WorkflowCommand::Show(args) => {
            let workflow = load_workflow(&root, &args.name)?;
            println!("{}", serde_json::to_string_pretty(&workflow)?);
        }
        WorkflowCommand::Run(args) => {
            let workflow = load_workflow(&root, &args.name)?;
            run_custom_workflow(&root, &args.name, workflow, args.max_rounds)?;
            println!("workflow completed: {}", args.name);
        }
    }
    Ok(())
}

fn load_workflow(root: &Path, name: &str) -> Result<Workflow> {
    let config = load_or_default(root)?;
    if let Some(workflow) = config.workflows.get(name) {
        return Ok(workflow.clone());
    }
    let path = dialec_dir(root)
        .join("workflows")
        .join(format!("{name}.json"));
    fsutil::read_json(&path)
}

fn run_custom_workflow(
    root: &Path,
    name: &str,
    workflow: Workflow,
    max_rounds_override: Option<u32>,
) -> Result<()> {
    let config = load_or_default(root)?;
    log_timeline(
        root,
        json!({"event": "workflow-started", "workflow": name, "at": Utc::now()}),
    )?;
    for phase in workflow.phases {
        if phase.steps.is_empty() {
            drive(
                root,
                DriveOptions {
                    max_rounds: max_rounds_override.or(phase.max_rounds),
                    max_parallel: 1,
                    phase: Some(phase.name.clone()),
                    no_cleanup: false,
                    pane: false,
                },
            )?;
            continue;
        }
        let max_rounds = max_rounds_override
            .or(phase.max_rounds)
            .unwrap_or(config.convergence.max_rounds)
            .max(1);
        for round in 1..=max_rounds {
            let mut latest = None;
            for step in &phase.steps {
                let harness = step
                    .harness
                    .clone()
                    .or_else(|| config.roles.get(&step.role).cloned())
                    .ok_or_else(|| anyhow::anyhow!("no harness mapping for role {}", step.role))?;
                let workspace = match &step.workspace {
                    Some(path) => fsutil::normalize(path)?,
                    None => root.to_path_buf(),
                };
                let artifacts = step
                    .artifacts
                    .iter()
                    .map(|path| fsutil::normalize(path))
                    .collect::<Result<Vec<_>>>()?;
                let tx = run_transaction(RunRequest {
                    phase: phase.name.clone(),
                    role: step.role.clone(),
                    harness,
                    task: format!(
                        "{}\n\nWorkflow `{name}`, phase `{}`, round {round}.",
                        step.task, phase.name
                    ),
                    workspace,
                    project_root: root.to_path_buf(),
                    pod: None,
                    sandbox: step.sandbox.clone(),
                    approval: step.approval.clone(),
                    timeout_ms: 1_800_000,
                    artifacts,
                    max_budget_usd: Some(config.budget.per_turn_max_usd),
                    max_turns: config.budget.max_turns,
                    pane: false,
                })?;
                log_timeline(
                    root,
                    json!({
                        "event": "workflow-turn-completed",
                        "workflow": name,
                        "phase": phase.name,
                        "round": round,
                        "transactionId": tx.id,
                        "role": tx.role,
                        "verdict": tx.signal.verdict,
                        "at": tx.completed_at
                    }),
                )?;
                latest = Some(tx);
            }
            if !phase.repeat_until_converged {
                break;
            }
            if let Some(tx) = latest
                && signal_converged(&tx.signal)
                && Ledger::read(root)?
                    .open_blocking(&phase.name, None)
                    .is_empty()
            {
                break;
            }
            if round == max_rounds {
                anyhow::bail!("workflow `{name}` phase `{}` did not converge", phase.name);
            }
        }
    }
    log_timeline(
        root,
        json!({"event": "workflow-completed", "workflow": name, "at": Utc::now()}),
    )?;
    Ok(())
}

fn cmd_resume(root: PathBuf, args: ResumeArgs) -> Result<()> {
    let state = read_state_refreshed(&root)?;
    if let Some(session_id) = args.session_id
        && session_id != state.session_id
    {
        anyhow::bail!(
            "requested session {} but local session is {}",
            session_id,
            state.session_id
        );
    }
    println!("resumed session {}", state.session_id);
    println!("phase: {}", state.current_phase);
    Ok(())
}

fn read_state_refreshed(root: &Path) -> Result<model::DialecState> {
    let mut state = read_state(root)?;
    if let Some(mut coordinator) = state.coordinator.clone()
        && coordinator.status == "running"
        && !pid_is_alive(coordinator.pid)
    {
        coordinator.status = "exited".to_string();
        state.coordinator = Some(coordinator.clone());
        write_state(root, &state)?;
        log_timeline(
            root,
            json!({
                "event": "coordinator-exited",
                "pid": coordinator.pid,
                "at": Utc::now()
            }),
        )?;
    }
    Ok(state)
}

fn pid_is_alive(pid: u32) -> bool {
    ProcessCommand::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn print_harness_table(reports: &BTreeMap<String, HarnessCapabilities>) {
    println!(
        "{:<10} {:<10} {:<12} {:<24} command",
        "harness", "available", "structured", "version"
    );
    for (name, report) in reports {
        println!(
            "{:<10} {:<10} {:<12} {:<24} {}",
            name,
            report.available,
            report.structured_output.supported,
            report.version.clone().unwrap_or_else(|| "-".to_string()),
            report.command.clone().unwrap_or_else(|| "-".to_string())
        );
    }
}

fn validate_role_mappings(
    root: &Path,
    reports: &BTreeMap<String, HarnessCapabilities>,
) -> Result<Vec<String>> {
    let config = load_or_default(root)?;
    let mut errors = vec![];
    for (role, harness) in config.roles {
        let Some(report) = reports.get(&harness) else {
            errors.push(format!("role {role} maps to unknown harness {harness}"));
            continue;
        };
        if !report.available {
            errors.push(format!("role {role} maps to unavailable harness {harness}"));
            continue;
        }
        if !report.headless {
            errors.push(format!("role {role} harness {harness} lacks headless mode"));
        }
        if !report.structured_output.supported {
            errors.push(format!(
                "role {role} harness {harness} lacks structured output support"
            ));
        }
        if matches!(role.as_str(), "implementer" | "refactorer") && report.cwd_flag.is_none() {
            errors.push(format!(
                "role {role} harness {harness} lacks workspace routing"
            ));
        }
    }
    Ok(errors)
}
