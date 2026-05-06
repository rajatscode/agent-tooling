use crate::config::{default_config, load_or_default, write_default_if_missing};
use crate::fsutil::{
    acquire_lock, append_line, dialec_dir, ensure_dir, read_json, write_json_pretty,
};
use crate::model::{BudgetConfig, Config, DialecState};
use crate::schema::signal_schema;
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub fn ensure_layout(root: &Path) -> Result<()> {
    let dialec = dialec_dir(root);
    for rel in [
        "capabilities",
        "locks",
        "roles",
        "skills",
        "session/turns",
        "session/integrations",
        "session/gates",
        "session/reminders",
        "session/phase-spec",
        "session/phase-impl",
        "session/phase-cleanup",
        "workspaces",
        "log",
        "memory",
        "scratch",
    ] {
        ensure_dir(&dialec.join(rel))?;
    }
    write_default_if_missing(root)?;
    write_signal_schema(root)?;
    write_default_roles(root, false)?;
    write_default_skills(root, false)?;
    write_memory_files(root)?;
    ensure_empty_file(&dialec.join("session").join("objections.jsonl"))?;
    ensure_empty_file(&dialec.join("log").join("timeline.jsonl"))?;
    ensure_empty_file(&dialec.join("log").join("decisions.jsonl"))?;
    ensure_empty_file(&dialec.join("log").join("costs.jsonl"))?;
    Ok(())
}

pub fn start_session(
    root: &Path,
    mode: &str,
    goal: Option<String>,
    budget: Option<String>,
) -> Result<DialecState> {
    ensure_layout(root)?;
    let budget_cfg = budget_config(budget);
    let session_id = Uuid::new_v4().to_string();
    let mut phases = BTreeMap::new();
    phases.insert(
        "spec".to_string(),
        json!({"status": "in-progress", "rounds": 0, "cost": 0.0}),
    );
    phases.insert(
        "implement".to_string(),
        json!({"status": "pending", "pods": {}}),
    );
    phases.insert("cleanup".to_string(), json!({"status": "pending"}));
    let state = DialecState {
        session_id,
        mode: mode.to_string(),
        started_at: Utc::now(),
        current_phase: "spec".to_string(),
        goal: goal.clone(),
        phases,
        total_cost: 0.0,
        budget: budget_cfg,
        coordinator: None,
        total_turns: 0,
    };

    write_state(root, &state)?;
    if let Some(goal) = goal {
        fs::write(
            dialec_dir(root)
                .join("session")
                .join("phase-spec")
                .join("goal.md"),
            format!("# Goal\n\n{goal}\n"),
        )?;
    }
    log_timeline(
        root,
        json!({
            "event": "session-started",
            "sessionId": state.session_id,
            "mode": state.mode,
            "phase": state.current_phase,
            "at": Utc::now()
        }),
    )?;
    Ok(state)
}

pub fn state_path(root: &Path) -> PathBuf {
    dialec_dir(root).join("dialec.json")
}

pub fn read_state(root: &Path) -> Result<DialecState> {
    read_json(&state_path(root))
}

pub fn write_state(root: &Path, state: &DialecState) -> Result<()> {
    let path = state_path(root);
    let mut next = state.clone();
    if path.exists()
        && let Ok(existing) = read_json::<DialecState>(&path)
        && existing.session_id == next.session_id
    {
        next.total_turns = next.total_turns.max(existing.total_turns);
        next.total_cost = next.total_cost.max(existing.total_cost);
        if next.coordinator.is_none() {
            next.coordinator = existing.coordinator;
        }
    }
    write_json_pretty(&path, &next)
}

pub fn log_timeline(root: &Path, value: Value) -> Result<()> {
    let _lock = acquire_lock(root, "timeline-log")?;
    append_line(
        &dialec_dir(root).join("log").join("timeline.jsonl"),
        &serde_json::to_string(&value)?,
    )
}

pub fn log_decision(root: &Path, value: Value) -> Result<()> {
    let _lock = acquire_lock(root, "decision-log")?;
    append_line(
        &dialec_dir(root).join("log").join("decisions.jsonl"),
        &serde_json::to_string(&value)?,
    )
}

pub fn log_cost(root: &Path, value: Value) -> Result<()> {
    let _lock = acquire_lock(root, "cost-log")?;
    append_line(
        &dialec_dir(root).join("log").join("costs.jsonl"),
        &serde_json::to_string(&value)?,
    )
}

pub fn enforce_budget(root: &Path, phase: Option<&str>) -> Result<()> {
    let state_path = state_path(root);
    if !state_path.exists() {
        return Ok(());
    }
    let state = read_state(root)?;
    if let Some(max_turns) = state.budget.max_turns
        && state.total_turns >= max_turns
    {
        anyhow::bail!(
            "budget exceeded: {} recorded turn(s), max {}",
            state.total_turns,
            max_turns
        );
    }
    if let Some(max_hours) = state.budget.max_hours {
        let elapsed_hours = (Utc::now() - state.started_at).num_seconds() as f64 / 3600.0;
        if elapsed_hours >= max_hours {
            anyhow::bail!(
                "budget exceeded: {:.2} elapsed hour(s), max {:.2}",
                elapsed_hours,
                max_hours
            );
        }
    }
    if state.total_cost >= state.budget.max_usd {
        anyhow::bail!(
            "budget exceeded: ${:.4} recorded, max ${:.4}",
            state.total_cost,
            state.budget.max_usd
        );
    }
    if let Some(deadline) = state.budget.deadline {
        if Utc::now() >= deadline {
            anyhow::bail!(
                "deadline reached: {}",
                deadline.format("%Y-%m-%d %H:%M %Z")
            );
        }
    }
    if let (Some(phase), Some(max_phase_usd)) = (phase, state.budget.per_phase_max_usd) {
        let phase_cost = phase_cost(root, phase)?;
        if phase_cost >= max_phase_usd {
            anyhow::bail!(
                "phase budget exceeded for {phase}: ${:.4} recorded, max ${:.4}",
                phase_cost,
                max_phase_usd
            );
        }
    }
    Ok(())
}

/// Returns true if there's still time remaining before the work_until deadline.
pub fn has_time_remaining(root: &Path) -> Result<bool> {
    let state = read_state(root)?;
    match state.budget.work_until {
        Some(until) => Ok(Utc::now() < until),
        None => Ok(false),
    }
}

fn phase_cost(root: &Path, phase: &str) -> Result<f64> {
    let path = dialec_dir(root).join("log").join("costs.jsonl");
    if !path.exists() {
        return Ok(0.0);
    }
    let content = fs::read_to_string(path)?;
    Ok(content
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| value.get("phase").and_then(Value::as_str) == Some(phase))
        .filter_map(|value| value.get("reportedCostUsd").and_then(Value::as_f64))
        .sum())
}

pub fn record_turn_cost(root: &Path, cost_usd: Option<f64>) -> Result<()> {
    let _lock = acquire_lock(root, "state")?;
    let path = state_path(root);
    if !path.exists() {
        return Ok(());
    }
    let mut state = read_state(root)?;
    state.total_turns = state.total_turns.saturating_add(1);
    if let Some(cost_usd) = cost_usd {
        state.total_cost += cost_usd;
    }
    write_state(root, &state)
}

pub fn reminder_text(
    root: &Path,
    role: &str,
    phase: &str,
    pod: Option<&str>,
) -> Result<Option<String>> {
    let config = load_or_default(root)?;
    let reminders = config.reminders;
    if !reminders.enabled {
        return Ok(None);
    }
    let mut out = format!(
        "# Dialec Role Reminder\n\nRole: `{role}`\nPhase: `{phase}`\nScope: `{}`\n\n",
        pod.unwrap_or("global")
    );
    if !reminders.global_rules.is_empty() {
        out.push_str("## Global Rules\n\n");
        for rule in reminders.global_rules {
            out.push_str(&format!("- {rule}\n"));
        }
        out.push('\n');
    }
    let role_rules = reminders
        .role_rules
        .get(role)
        .cloned()
        .unwrap_or_else(|| {
            vec![
                format!("Stay within the `{role}` role boundary."),
                "Do not perform work owned by another role; raise an objection or ask the coordinator/user when boundaries conflict.".to_string(),
            ]
        });
    out.push_str("## Role Boundary\n\n");
    for rule in role_rules {
        out.push_str(&format!("- {rule}\n"));
    }
    out.push_str(
        "\n## Output Contract\n\n- Produce or review the requested artifact.\n- Cite evidence.\n- End with the structured convergence signal.\n",
    );
    Ok(Some(out))
}

pub fn should_emit_reminder(root: &Path, turn_number: u32) -> Result<bool> {
    let config = load_or_default(root)?;
    if !config.reminders.enabled {
        return Ok(false);
    }
    let every = config.reminders.every_turns.max(1);
    Ok(turn_number == 1 || turn_number.is_multiple_of(every))
}

pub fn append_objections(root: &Path, values: &[Value]) -> Result<()> {
    let _lock = acquire_lock(root, "objections")?;
    let path = dialec_dir(root).join("session").join("objections.jsonl");
    for value in values {
        append_line(&path, &serde_json::to_string(value)?)?;
    }
    Ok(())
}

pub fn next_turn_dir(root: &Path, harness: &str, role: &str) -> Result<(String, PathBuf)> {
    let _lock = acquire_lock(root, "turns")?;
    let turns = dialec_dir(root).join("session").join("turns");
    ensure_dir(&turns)?;
    let mut max_id = 0u32;
    for entry in fs::read_dir(&turns)? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(|s| s.to_string()) else {
            continue;
        };
        if let Some(prefix) = name.split('-').next()
            && let Ok(num) = prefix.parse::<u32>()
        {
            max_id = max_id.max(num);
        }
    }
    let mut next_id = max_id + 1;
    loop {
        let id = format!("{next_id:04}-{}-{}", sanitize(harness), sanitize(role));
        let dir = turns.join(&id);
        match fs::create_dir(&dir) {
            Ok(()) => return Ok((id, dir)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                next_id += 1;
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to create turn directory {}", dir.display()));
            }
        }
    }
}

pub fn signal_schema_path(root: &Path) -> PathBuf {
    dialec_dir(root).join("signal-schema.json")
}

pub fn role_path(root: &Path, role: &str) -> PathBuf {
    dialec_dir(root)
        .join("roles")
        .join(format!("{}.md", sanitize(role)))
}

fn write_signal_schema(root: &Path) -> Result<()> {
    let path = signal_schema_path(root);
    if !path.exists() {
        write_json_pretty(&path, &signal_schema())?;
    }
    Ok(())
}

pub fn refresh_default_prompts(root: &Path) -> Result<()> {
    write_default_roles(root, true)?;
    write_default_skills(root, true)
}

fn write_default_roles(root: &Path, force: bool) -> Result<()> {
    let config = default_config();
    for role in config.roles.keys() {
        let path = role_path(root, role);
        if path.exists() && !force {
            continue;
        }
        let body = role_prompt(role, &config);
        fs::write(path, body)?;
    }
    Ok(())
}

fn write_default_skills(root: &Path, force: bool) -> Result<()> {
    let skills_dir = dialec_dir(root).join("skills");
    let sidecar = skills_dir.join("sidecar.md");
    if force || !sidecar.exists() {
        fs::write(&sidecar, sidecar_skill())?;
    }
    let coordinator = skills_dir.join("coordinator.md");
    if force || !coordinator.exists() {
        fs::write(&coordinator, coordinator_skill())?;
    }
    Ok(())
}

fn write_memory_files(root: &Path) -> Result<()> {
    let memory_dir = dialec_dir(root).join("memory");
    for (name, title) in [
        ("project.md", "Project Memory"),
        ("decisions.md", "Decision Memory"),
        ("patterns.md", "Pattern Memory"),
        ("gotchas.md", "Gotchas"),
        ("user-prefs.md", "User Preferences"),
    ] {
        let path = memory_dir.join(name);
        if !path.exists() {
            fs::write(path, format!("# {title}\n\n"))?;
        }
    }
    Ok(())
}

fn ensure_empty_file(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::write(path, "")?;
    }
    Ok(())
}

pub fn budget_config(budget: Option<String>) -> BudgetConfig {
    let mut cfg = default_config().budget;
    let Some(raw) = budget else { return cfg };
    let raw = raw.trim();

    // Support multiple constraints separated by commas: "$10, until 8am ET"
    for part in raw.split(',') {
        parse_budget_part(part.trim(), &mut cfg);
    }
    // If only "until" is set without a separate deadline, use work_until as deadline too
    if cfg.deadline.is_none() {
        cfg.deadline = cfg.work_until;
    }
    cfg
}

fn parse_budget_part(part: &str, cfg: &mut BudgetConfig) {
    // "$10" or "$10.50"
    if let Some(stripped) = part.strip_prefix('$') {
        if let Ok(value) = stripped.parse::<f64>() {
            cfg.max_usd = value;
        }
        return;
    }

    // "4h" or "0.5h"
    if let Some(stripped) = part.strip_suffix('h') {
        if let Ok(hours) = stripped.trim().parse::<f64>() {
            cfg.max_hours = Some(hours);
            let deadline = Utc::now() + chrono::Duration::seconds((hours * 3600.0) as i64);
            cfg.deadline = Some(deadline);
        }
        return;
    }

    // "10turns" or "10 turns"
    if let Some(stripped) = part.strip_suffix("turns").or_else(|| part.strip_suffix("turn")) {
        if let Ok(value) = stripped.trim().parse::<u32>() {
            cfg.max_turns = Some(value);
        }
        return;
    }

    // "until 8am ET", "until 8am", "until 6:30am ET", "until 22:00"
    if let Some(time_str) = part.strip_prefix("until ").or_else(|| part.strip_prefix("until:")) {
        if let Some(deadline) = parse_until_time(time_str.trim()) {
            cfg.work_until = Some(deadline);
            cfg.deadline = Some(deadline);
        }
        return;
    }
}

fn parse_until_time(s: &str) -> Option<chrono::DateTime<Utc>> {
    use chrono::{Local, NaiveTime, TimeZone, FixedOffset};

    // Strip timezone suffix if present
    let (time_part, tz_offset_hours) = if s.ends_with(" ET") || s.ends_with(" EST") {
        (s.trim_end_matches(" ET").trim_end_matches(" EST"), -5)
    } else if s.ends_with(" EDT") {
        (s.trim_end_matches(" EDT"), -4)
    } else if s.ends_with(" CT") || s.ends_with(" CST") {
        (s.trim_end_matches(" CT").trim_end_matches(" CST"), -6)
    } else if s.ends_with(" PT") || s.ends_with(" PST") {
        (s.trim_end_matches(" PT").trim_end_matches(" PST"), -8)
    } else if s.ends_with(" PDT") {
        (s.trim_end_matches(" PDT"), -7)
    } else if s.ends_with(" MT") || s.ends_with(" MST") {
        (s.trim_end_matches(" MT").trim_end_matches(" MST"), -7)
    } else if s.ends_with(" UTC") || s.ends_with(" GMT") {
        (s.trim_end_matches(" UTC").trim_end_matches(" GMT"), 0)
    } else {
        // Assume local timezone
        let local_offset = Local::now().offset().local_minus_utc() / 3600;
        (s, local_offset)
    };

    let time_part = time_part.trim();

    // Parse various time formats
    let naive_time = parse_naive_time(time_part)?;

    let tz = FixedOffset::east_opt(tz_offset_hours * 3600)?;
    let today = Utc::now().with_timezone(&tz).date_naive();
    let naive_dt = today.and_time(naive_time);
    let target = tz.from_local_datetime(&naive_dt).single()?;
    let target_utc = target.with_timezone(&Utc);

    // If the target is in the past, assume tomorrow
    if target_utc <= Utc::now() {
        Some(target_utc + chrono::Duration::hours(24))
    } else {
        Some(target_utc)
    }
}

fn parse_naive_time(s: &str) -> Option<chrono::NaiveTime> {
    use chrono::NaiveTime;

    // "8am", "8pm", "8:30am", "11pm", "8:30 am"
    let s = s.replace(' ', "").to_lowercase();

    if let Some(before_am) = s.strip_suffix("am") {
        let parts: Vec<&str> = before_am.split(':').collect();
        let hour = parts[0].parse::<u32>().ok()?;
        let hour = if hour == 12 { 0 } else { hour };
        let min = parts.get(1).and_then(|m| m.parse::<u32>().ok()).unwrap_or(0);
        return NaiveTime::from_hms_opt(hour, min, 0);
    }

    if let Some(before_pm) = s.strip_suffix("pm") {
        let parts: Vec<&str> = before_pm.split(':').collect();
        let hour = parts[0].parse::<u32>().ok()?;
        let hour = if hour == 12 { hour } else { hour + 12 };
        let min = parts.get(1).and_then(|m| m.parse::<u32>().ok()).unwrap_or(0);
        return NaiveTime::from_hms_opt(hour, min, 0);
    }

    // "22:00", "8:30"
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() == 2 {
        let hour = parts[0].parse::<u32>().ok()?;
        let min = parts[1].parse::<u32>().ok()?;
        return NaiveTime::from_hms_opt(hour, min, 0);
    }

    None
}

fn role_prompt(role: &str, config: &Config) -> String {
    let role_rules = config
        .reminders
        .role_rules
        .get(role)
        .cloned()
        .unwrap_or_else(|| {
            vec![
                format!("Stay strictly within the `{role}` responsibility boundary."),
                "When in doubt, ask the coordinator/user or raise an objection instead of taking over another role.".to_string(),
            ]
        });
    let mut out = format!(
        "# {role}\n\nYou are acting as `{role}` in a Dialec session.\n\n\
Dialec coordinates multi-agent work through artifacts, worktrees, immutable transactions, and convergence signals. \
Your output must preserve auditability and end with the required structured convergence signal.\n\n\
## Role Boundary\n\n"
    );
    for rule in role_rules {
        out.push_str(&format!("- {rule}\n"));
    }
    out.push_str(
        "\n## Protocol\n\n\
- Read the task, input artifacts, memory, reminder, and open objection ledger before acting.\n\
- Use stable objection ids until they are resolved, withdrawn, or user-accepted.\n\
- Cite concrete evidence: file paths, commands, transaction ids, artifact paths, or spec sections.\n\
- If you cannot prove convergence, return `reject` with blocking objections.\n\
- Do not mutate files outside your assigned workspace and role.\n",
    );
    out
}

fn sidecar_skill() -> &'static str {
    r#"# Dialec Sidecar Skill

You are the coordinator in a Dialec sidecar session. You think with the user and dispatch work to other harnesses (Codex, Gemini, Cursor) via `dialec`. You are never blocked waiting for a child — children run in tmux panes and you get notified when they finish.

## Spawning Children (NON-BLOCKING)

CRITICAL: Always use `--pane` to spawn children non-blocking. Never use `dialec run` without `--pane` — it blocks you.

**Pattern for every child dispatch:**

1. Spawn the child (returns immediately):
```bash
dialec run --harness codex --role spec-reviewer --phase spec \
  --task "Review this spec..." --artifact path/to/spec.md \
  --sandbox read-only --timeout-seconds 120 --pane
```

2. Immediately start a background watcher (use `run_in_background: true`):
```bash
TURN=0001-codex-spec-reviewer
while [ ! -f .dialec/session/turns/$TURN/signal.json ] || grep -q '"pending"' .dialec/session/turns/$TURN/signal.json 2>/dev/null; do sleep 2; done && dialec inbox coordinator
```

3. Continue working — talk to the user, spawn more children, do whatever.

4. When the background watcher completes, read the result from its output. The coordinator inbox has the child's verdict and summary.

**Spawning multiple children in parallel:** Run steps 1-2 for each child. Each gets its own background watcher. You stay unblocked.

## Sending Messages to Children

- `dialec send --to <role> "message"` — send a directive/question to a child's inbox
- `dialec send --to <role> --kind question "..."` — typed: directive, question, update, cancel, nudge
- `dialec send --to <role> --ping "..."` — also nudges the child's tmux pane
- Messages are injected into the child's prompt at the start of their next turn

## Reading Child Responses

- `dialec inbox coordinator` — read all messages from children
- `dialec inbox <role>` — read messages TO a specific role
- Children write to the coordinator channel when they finish (via `dialec finalize`)

## Commands

- `dialec run --pane ...` — spawn a non-blocking child in a tmux pane
- `dialec send --to <role> "msg"` — send message to child
- `dialec inbox coordinator` — read child responses
- `dialec status` — session state
- `dialec log --phase spec` — timeline events
- `dialec worktree create/remove <name>` — manage worktrees
- `dialec advance --reason "..."` — force past deadlock (user decision only)
- `dialec release` — hand off to autonomous headless coordinator
- `dialec drive` / `dialec spec` / `dialec implement` / `dialec cleanup` — deterministic local autopilot (blocks until done)

## Phase Workflow

1. Co-author the spec with the user.
2. Spawn adversarial review child (`--pane`), start background watcher.
3. When watcher returns, read objections from `dialec inbox coordinator`.
4. Present objections to user. Iterate until converged.
5. Implementation: create worktrees, spawn implementer children in parallel.
6. Spawn verifier/meta-verifier/deslopper children as each pod converges.
7. Merge converged pods. Run cleanup/refactor phase.

## Convergence

Converged = latest signal verdict is `approve` or `approve-with-nits` AND no open blocking objections in `.dialec/session/objections.jsonl`. Never force convergence — use `dialec advance --reason` only for explicit user decisions.

## Role Discipline

PM/coordinator/spec/review roles do not write source code. Implementer/refactorer roles own code changes. Verifier/adversary worktrees are disposable.
"#
}

fn coordinator_skill() -> &'static str {
    r#"# Dialec Autonomous Coordinator Skill

You are the headless coordinator for a Dialec session. You have Bash, Read, Glob, Grep, Edit, and Write tool access. Dialec is the system of record for transactions, worktrees, ledgers, budget, reminders, and phase state.

## Primary Loop

1. Run `dialec status` and inspect `.dialec/dialec.json`, `.dialec/config.json`, `.dialec/session/objections.jsonl`, and relevant artifacts.
2. Before assigning work, call `dialec cron tick --role <role>` or read the reminder injected into each transaction. Enforce role boundaries.
3. Drive phases spec -> implement -> cleanup. You may use `dialec drive` for deterministic end-to-end autopilot, or call `dialec run`/`dialec worktree` primitives directly when steering is needed.
4. Check convergence after every review: latest transaction succeeded, structured signal approves or approves-with-nits, and no scoped open blocking objections remain.
5. Fail closed on correctness, security, data-loss, migration, operability, and test-coverage blockers.
6. On deadlock, write `.dialec/session/escalation.md`, include transaction ids and open blockers, and exit non-zero.
7. When complete, write `.dialec/session/final-report.md` with the final state, integrated changes, verification evidence, residual risks, and deferred nits.

## Boundaries

Coordinate, dispatch, read artifacts, and make convergence decisions. Do not directly implement source changes while acting as coordinator. Use implementer/refactorer roles for accepted code changes and verifier/adversary roles for disposable validation.

## Audit

Every material action must go through Dialec commands or be written into `.dialec/log/decisions.jsonl` via the appropriate command. Do not rely on memory outside `.dialec/`.
"#
}

pub fn sanitize(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}
