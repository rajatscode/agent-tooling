use crate::capabilities::probe_harness;
use crate::fsutil::{
    acquire_lock, dialec_dir, ensure_dir, relative_to, sha256_bytes, sha256_file, write_json_pretty,
};
use crate::git::{snapshot, tracked_diff};
use crate::model::{
    ArtifactRef, CommandInvocation, ConvergenceSignal, CostRecord, HarnessError, RunRequest,
    RunTransaction,
};
use crate::schema::{extract_json, fallback_reject_signal, parse_signal};
use crate::session::{
    append_objections, enforce_budget, log_timeline, next_turn_dir, record_turn_cost,
    reminder_text, role_path, should_emit_reminder, signal_schema_path,
};
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub fn run_transaction(req: RunRequest) -> Result<RunTransaction> {
    ensure_dir(&dialec_dir(&req.project_root))?;
    enforce_budget(&req.project_root, Some(&req.phase))?;
    let capabilities = probe_harness(&req.harness, &req.project_root)?;
    if !capabilities.available {
        anyhow::bail!(
            "harness {} is unavailable: {}",
            req.harness,
            capabilities.limitations.join("; ")
        );
    }

    let (turn_id, turn_dir) = next_turn_dir(&req.project_root, &req.harness, &req.role)?;
    let started_at = Utc::now();
    let role_file = role_path(&req.project_root, &req.role);
    let schema_file = signal_schema_path(&req.project_root);
    let final_message_path = turn_dir.join("final-message.md");
    let resume_from = select_resume_session(&req, capabilities.can_resume)?;
    let reminder_path = turn_dir.join("reminder.md");
    let turn_number = turn_id
        .split('-')
        .next()
        .and_then(|prefix| prefix.parse::<u32>().ok())
        .unwrap_or(1);
    let reminder_artifact = if should_emit_reminder(&req.project_root, turn_number)? {
        let text = reminder_text(&req.project_root, &req.role, &req.phase, req.pod.as_deref())?
            .unwrap_or_default();
        fs::write(&reminder_path, text)?;
        log_timeline(
            &req.project_root,
            json!({
                "event": "role-reminder",
                "transactionId": turn_id,
                "phase": req.phase,
                "pod": req.pod,
                "role": req.role,
                "path": relative_to(&reminder_path, &req.project_root),
                "at": Utc::now()
            }),
        )?;
        Some(reminder_path.clone())
    } else {
        None
    };
    let task = compose_task(&req, &role_file, reminder_artifact.as_deref())?;
    let before = snapshot(&req.workspace);

    let (mut program, args, cwd) = build_command(
        &req,
        &task,
        &schema_file,
        &role_file,
        &final_message_path,
        resume_from.as_deref(),
    )?;
    if let Some(command) = capabilities.command.as_ref() {
        program = command.clone();
    }
    let input_artifacts = input_artifact_refs(&req, &role_file, reminder_artifact.as_deref())?;
    write_json_pretty(
        &turn_dir.join("input.json"),
        &json!({
            "phase": req.phase,
            "pod": req.pod,
            "role": req.role,
            "harness": req.harness,
            "workspace": req.workspace,
            "artifacts": input_artifacts,
            "rolePrompt": role_file,
            "reminder": reminder_artifact,
            "sandbox": req.sandbox,
            "approval": req.approval,
            "resumeFrom": resume_from,
            "timeoutMs": req.timeout_ms,
            "budgets": {
                "maxBudgetUsd": req.max_budget_usd,
                "maxTurns": req.max_turns
            }
        }),
    )?;

    let mut argv = vec![program.clone()];
    argv.extend(args.clone());
    let command = CommandInvocation {
        argv,
        cwd: cwd.to_string_lossy().to_string(),
        timeout_ms: req.timeout_ms,
        env_allowlist: BTreeMap::new(),
    };
    write_json_pretty(&turn_dir.join("command.json"), &command)?;
    write_json_pretty(&turn_dir.join("before.json"), &before)?;

    let output = run_external(&program, &args, &cwd, req.timeout_ms)?;
    fs::write(turn_dir.join("stdout.log"), &output.stdout)?;
    fs::write(turn_dir.join("stderr.log"), &output.stderr)?;

    let events_path = turn_dir.join("events.jsonl");
    if let Some(events) = normalize_events(&req.harness, &output.stdout) {
        fs::write(&events_path, events)?;
    }

    if !final_message_path.exists() {
        fs::write(&final_message_path, &output.stdout)?;
    }

    let final_message = fs::read_to_string(&final_message_path).unwrap_or_default();
    let stdout_text = String::from_utf8_lossy(&output.stdout);
    let structured_value = extract_signal_from_text(&final_message)
        .or_else(|| extract_last_jsonl_message(&stdout_text))
        .or_else(|| extract_signal_from_text(&stdout_text));

    let (structured_value, signal, signal_error) = match structured_value {
        Some(value) => match parse_signal(&value) {
            Ok(signal) => (value, signal, None),
            Err(error) => {
                let signal = fallback_reject_signal(
                    "dialec-invalid-structured-signal",
                    format!("invalid structured convergence signal: {error}"),
                );
                (
                    json!({"parseError": error.to_string()}),
                    signal,
                    Some(error.to_string()),
                )
            }
        },
        None => {
            let signal = fallback_reject_signal(
                "dialec-missing-structured-signal",
                "missing structured convergence signal",
            );
            (
                json!(null),
                signal,
                Some("missing structured convergence signal".to_string()),
            )
        }
    };

    write_json_pretty(&turn_dir.join("structured.json"), &structured_value)?;
    write_json_pretty(&turn_dir.join("signal.json"), &signal)?;

    let after = snapshot(&req.workspace);
    write_json_pretty(&turn_dir.join("after.json"), &after)?;
    fs::write(turn_dir.join("patch.diff"), tracked_diff(&req.workspace))?;
    let cost = extract_cost(&structured_value, &stdout_text, &final_message);
    let session_id = extract_session_id(&structured_value, &stdout_text, &final_message);

    let completed_at = Utc::now();
    let exit_code = output.exit_code;
    let mut error = output.error;
    if error.is_none()
        && let Some(signal_error) = signal_error
    {
        error = Some(HarnessError {
            kind: "structured-output".to_string(),
            message: signal_error,
        });
    }
    if error.is_none()
        && let (Some(cost), Some(max_budget_usd)) = (
            cost.as_ref().and_then(|record| record.usd),
            req.max_budget_usd,
        )
        && cost > max_budget_usd
    {
        error = Some(HarnessError {
            kind: "budget".to_string(),
            message: format!("turn reported ${cost:.4}, above per-turn cap ${max_budget_usd:.4}"),
        });
    }

    let transaction = RunTransaction {
        id: turn_id.clone(),
        phase: req.phase.clone(),
        pod: req.pod.clone(),
        role: req.role.clone(),
        harness: req.harness.clone(),
        harness_version: capabilities.version,
        workspace: req.workspace.to_string_lossy().to_string(),
        started_at,
        completed_at,
        command,
        input_artifacts,
        before,
        after,
        stdout: artifact_ref(&turn_dir.join("stdout.log"), &req.project_root, "stdout")?,
        stderr: artifact_ref(&turn_dir.join("stderr.log"), &req.project_root, "stderr")?,
        event_log: if events_path.exists() {
            Some(artifact_ref(&events_path, &req.project_root, "event-log")?)
        } else {
            None
        },
        final_message: artifact_ref(&final_message_path, &req.project_root, "final-message")?,
        structured: artifact_ref(
            &turn_dir.join("structured.json"),
            &req.project_root,
            "report",
        )?,
        signal: signal.clone(),
        patch: artifact_ref(&turn_dir.join("patch.diff"), &req.project_root, "patch")?,
        cost: cost.clone(),
        resume_from,
        session_id: session_id.clone(),
        exit_code,
        error,
    };

    write_json_pretty(&turn_dir.join("transaction.json"), &transaction)?;
    if let Some(session_id) = session_id {
        record_resume_session(&req, &transaction.id, &session_id)?;
    }
    append_objections(&req.project_root, &ledger_entries(&transaction, &signal))?;
    record_turn_cost(
        &req.project_root,
        cost.as_ref().and_then(|record| record.usd),
    )?;

    Ok(transaction)
}

fn compose_task(
    req: &RunRequest,
    role_file: &Path,
    reminder_file: Option<&Path>,
) -> Result<String> {
    let mut out = String::new();
    out.push_str("# Dialec Turn\n\n");
    out.push_str("You are running inside Dialec. Produce the requested artifact and end with a structured convergence signal matching `.dialec/signal-schema.json`.\n\n");

    if role_file.exists() {
        out.push_str("## Role Prompt\n\n");
        out.push_str(&fs::read_to_string(role_file).unwrap_or_default());
        out.push_str("\n\n");
    }

    if let Some(reminder_file) = reminder_file
        && reminder_file.exists()
    {
        out.push_str("## Dialec Role Reminder\n\n");
        out.push_str(&fs::read_to_string(reminder_file).unwrap_or_default());
        out.push_str("\n\n");
    }

    out.push_str("## Persistent Memory\n\n");
    for rel in [
        "project.md",
        "decisions.md",
        "patterns.md",
        "gotchas.md",
        "user-prefs.md",
    ] {
        let path = dialec_dir(&req.project_root).join("memory").join(rel);
        if path.exists() {
            out.push_str(&format!("### .dialec/memory/{rel}\n\n"));
            out.push_str(&fs::read_to_string(path).unwrap_or_default());
            out.push_str("\n\n");
        }
    }

    let ledger = dialec_dir(&req.project_root)
        .join("session")
        .join("objections.jsonl");
    if ledger.exists() {
        out.push_str("## Objection Ledger\n\n```jsonl\n");
        out.push_str(&fs::read_to_string(ledger).unwrap_or_default());
        out.push_str("\n```\n\n");
    }

    let scratch = dialec_dir(&req.project_root)
        .join("scratch")
        .join(&req.role);
    ensure_dir(&scratch)?;
    out.push_str(&format!(
        "## Private Scratchpad\n\nYou may use `{}` for private notes during this role. Do not treat scratchpad content as shared truth unless it is promoted to an artifact.\n\n",
        scratch.display()
    ));

    out.push_str("## Task\n\n");
    out.push_str(&req.task);
    out.push_str("\n\n");

    if !req.artifacts.is_empty() {
        out.push_str("## Input Artifacts\n\n");
        for artifact in &req.artifacts {
            out.push_str(&format!("### {}\n\n", artifact.display()));
            match fs::read_to_string(artifact) {
                Ok(content) => {
                    out.push_str("```text\n");
                    out.push_str(&content);
                    if !content.ends_with('\n') {
                        out.push('\n');
                    }
                    out.push_str("```\n\n");
                }
                Err(_) => {
                    out.push_str("[binary or unreadable artifact omitted from prompt body]\n\n");
                }
            }
        }
    }

    Ok(out)
}

fn build_command(
    req: &RunRequest,
    task: &str,
    schema_file: &Path,
    role_file: &Path,
    final_message_path: &Path,
    resume_from: Option<&str>,
) -> Result<(String, Vec<String>, PathBuf)> {
    match req.harness.as_str() {
        "claude" => {
            let mut args = vec![
                "-p".to_string(),
                task.to_string(),
                "--bare".to_string(),
                "--output-format".to_string(),
                "json".to_string(),
                "--json-schema".to_string(),
                schema_file.to_string_lossy().to_string(),
            ];
            if role_file.exists() {
                args.push("--append-system-prompt-file".to_string());
                args.push(role_file.to_string_lossy().to_string());
            }
            if let Some(max_budget_usd) = req.max_budget_usd {
                args.push("--max-budget-usd".to_string());
                args.push(format!("{max_budget_usd:.4}"));
            }
            if let Some(session_id) = resume_from {
                args.push("--resume".to_string());
                args.push(session_id.to_string());
            }
            Ok(("claude".to_string(), args, req.workspace.clone()))
        }
        "codex" => {
            let mut args = vec![
                "exec".to_string(),
                "--json".to_string(),
                "--sandbox".to_string(),
                req.sandbox.clone(),
                "--cd".to_string(),
                req.workspace.to_string_lossy().to_string(),
                "--output-schema".to_string(),
                schema_file.to_string_lossy().to_string(),
                "-o".to_string(),
                final_message_path.to_string_lossy().to_string(),
            ];
            if let Some(session_id) = resume_from {
                args.push("resume".to_string());
                args.push(session_id.to_string());
                args.push(task.to_string());
            } else {
                args.push(task.to_string());
            }
            Ok(("codex".to_string(), args, req.project_root.clone()))
        }
        "gemini" => {
            let mut args = vec![
                "-p".to_string(),
                task.to_string(),
                "--output-format".to_string(),
                "json".to_string(),
                "--approval-mode".to_string(),
                "yolo".to_string(),
            ];
            if let Some(session_id) = resume_from {
                args.push("--resume".to_string());
                args.push(session_id.to_string());
            }
            Ok(("gemini".to_string(), args, req.workspace.clone()))
        }
        "cursor" => {
            let mut args = vec![
                "-p".to_string(),
                task.to_string(),
                "--output-format".to_string(),
                "json".to_string(),
                "--force".to_string(),
            ];
            if let Some(session_id) = resume_from {
                args.push("--resume".to_string());
                args.push(session_id.to_string());
            }
            Ok(("cursor-agent".to_string(), args, req.workspace.clone()))
        }
        "claudish" => {
            let mut args = vec![
                "-p".to_string(),
                task.to_string(),
                "--output-format".to_string(),
                "json".to_string(),
            ];
            if let Some(session_id) = resume_from {
                args.push("--resume".to_string());
                args.push(session_id.to_string());
            }
            Ok(("claudish".to_string(), args, req.workspace.clone()))
        }
        other => anyhow::bail!("unsupported harness: {other}"),
    }
}

fn run_external(
    program: &str,
    args: &[String],
    cwd: &Path,
    timeout_ms: u64,
) -> Result<ExternalOutput> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn {program}"))?;

    let mut stdout = child
        .stdout
        .take()
        .context("failed to capture child stdout")?;
    let mut stderr = child
        .stderr
        .take()
        .context("failed to capture child stderr")?;
    let stdout_handle = thread::spawn(move || {
        let mut data = Vec::new();
        let _ = stdout.read_to_end(&mut data);
        data
    });
    let stderr_handle = thread::spawn(move || {
        let mut data = Vec::new();
        let _ = stderr.read_to_end(&mut data);
        data
    });

    let started = Instant::now();
    let timeout = Duration::from_millis(timeout_ms);
    loop {
        if let Some(status) = child.try_wait()? {
            let stdout = stdout_handle.join().unwrap_or_default();
            let stderr = stderr_handle.join().unwrap_or_default();
            return Ok(ExternalOutput {
                exit_code: status.code().unwrap_or(-1),
                stdout,
                stderr,
                error: if status.success() {
                    None
                } else {
                    Some(HarnessError {
                        kind: "process-exit".to_string(),
                        message: format!("{program} exited with {status}"),
                    })
                },
            });
        }
        if timeout_ms > 0 && started.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = stdout_handle.join().unwrap_or_default();
            let stderr = stderr_handle.join().unwrap_or_default();
            return Ok(ExternalOutput {
                exit_code: 124,
                stdout,
                stderr,
                error: Some(HarnessError {
                    kind: "timeout".to_string(),
                    message: format!("{program} exceeded timeout of {timeout_ms}ms"),
                }),
            });
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn input_artifact_refs(
    req: &RunRequest,
    role_file: &Path,
    reminder_file: Option<&Path>,
) -> Result<Vec<ArtifactRef>> {
    let mut paths = req.artifacts.clone();
    if role_file.exists() {
        paths.push(role_file.to_path_buf());
    }
    if let Some(reminder_file) = reminder_file
        && reminder_file.exists()
    {
        paths.push(reminder_file.to_path_buf());
    }
    paths
        .iter()
        .map(|path| artifact_ref(path, &req.project_root, "artifact"))
        .collect()
}

fn artifact_ref(path: &Path, root: &Path, artifact_type: &str) -> Result<ArtifactRef> {
    let sha = if path.exists() {
        sha256_file(path)?
    } else {
        sha256_bytes(b"")
    };
    Ok(ArtifactRef {
        id: uuid::Uuid::new_v4().to_string(),
        path: relative_to(path, root),
        sha256: sha,
        artifact_type: artifact_type.to_string(),
    })
}

fn extract_last_jsonl_message(stdout: &str) -> Option<Value> {
    for line in stdout.lines().rev() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(signal) = signal_candidate_from_value(&value) {
            return Some(signal);
        }
    }
    None
}

fn extract_signal_from_text(text: &str) -> Option<Value> {
    extract_json(text).and_then(|value| signal_candidate_from_value(&value))
}

fn signal_candidate_from_value(value: &Value) -> Option<Value> {
    if value.get("verdict").is_some() {
        return Some(value.clone());
    }
    match value {
        Value::Object(map) => {
            for key in ["result", "message", "text", "content", "finalMessage"] {
                if let Some(text) = map.get(key).and_then(Value::as_str)
                    && let Some(signal) = extract_signal_from_text(text)
                {
                    return Some(signal);
                }
            }
            for child in map.values() {
                if let Some(signal) = signal_candidate_from_value(child) {
                    return Some(signal);
                }
            }
            None
        }
        Value::Array(items) => items.iter().find_map(signal_candidate_from_value),
        _ => None,
    }
}

fn normalize_events(harness: &str, stdout: &[u8]) -> Option<Vec<u8>> {
    let text = String::from_utf8_lossy(stdout);
    if text.trim().is_empty() {
        return None;
    }
    let parsed = parse_stdout_values(&text);
    let mut records = vec![];
    if parsed.is_empty() {
        records.push(json!({
            "event": "harness-event",
            "harness": harness,
            "category": "text",
            "summary": first_chars(&text, 240),
            "rawText": text.to_string(),
            "at": Utc::now()
        }));
    } else {
        for (index, value) in parsed.iter().enumerate() {
            records.extend(normalize_harness_value(harness, index, value));
        }
    }
    if records.is_empty() {
        None
    } else {
        let mut out = String::new();
        for record in records {
            out.push_str(&serde_json::to_string(&record).ok()?);
            out.push('\n');
        }
        Some(out.into_bytes())
    }
}

fn parse_stdout_values(stdout: &str) -> Vec<Value> {
    let mut values = vec![];
    let mut all_lines_json = false;
    for line in stdout.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str::<Value>(line) {
            Ok(value) => {
                all_lines_json = true;
                values.push(value);
            }
            Err(_) => {
                all_lines_json = false;
                values.clear();
                break;
            }
        }
    }
    if all_lines_json && !values.is_empty() {
        return values;
    }
    serde_json::from_str::<Value>(stdout).into_iter().collect()
}

fn normalize_harness_value(harness: &str, index: usize, value: &Value) -> Vec<Value> {
    match harness {
        "claude" => normalize_claude_event(index, value),
        "codex" => normalize_codex_event(index, value),
        "gemini" => normalize_gemini_event(index, value),
        "cursor" => normalize_cursor_event(index, value),
        "claudish" => normalize_claudish_event(index, value),
        _ => vec![generic_event(harness, index, value)],
    }
}

fn normalize_claude_event(index: usize, value: &Value) -> Vec<Value> {
    let mut events = vec![generic_event("claude", index, value)];
    if let Some(session_id) = find_session_id(value) {
        events.push(json!({
            "event": "harness-event",
            "harness": "claude",
            "category": "session",
            "sessionId": session_id,
            "sourceIndex": index,
            "at": Utc::now()
        }));
    }
    if let Some(cost) = find_cost_value(value) {
        events.push(json!({
            "event": "harness-event",
            "harness": "claude",
            "category": "cost",
            "costUsd": cost,
            "sourceIndex": index,
            "at": Utc::now()
        }));
    }
    for (tool_name, input) in find_tool_calls(value) {
        events.push(json!({
            "event": "harness-event",
            "harness": "claude",
            "category": "tool-call",
            "toolName": tool_name,
            "input": input,
            "sourceIndex": index,
            "at": Utc::now()
        }));
    }
    events
}

fn normalize_codex_event(index: usize, value: &Value) -> Vec<Value> {
    let mut event = generic_event("codex", index, value);
    if let Some(command) = find_command(value) {
        event["category"] = json!("command");
        event["command"] = json!(command);
    }
    if let Some(path) = find_path(value) {
        event["path"] = json!(path);
    }
    let mut events = vec![event];
    if let Some(session_id) = find_session_id(value) {
        events.push(json!({
            "event": "harness-event",
            "harness": "codex",
            "category": "session",
            "sessionId": session_id,
            "sourceIndex": index,
            "at": Utc::now()
        }));
    }
    events
}

fn normalize_gemini_event(index: usize, value: &Value) -> Vec<Value> {
    let mut events = vec![generic_event("gemini", index, value)];
    for text in find_texts(value).into_iter().take(8) {
        events.push(json!({
            "event": "harness-event",
            "harness": "gemini",
            "category": "message",
            "text": text,
            "sourceIndex": index,
            "at": Utc::now()
        }));
    }
    for (tool_name, input) in find_tool_calls(value) {
        events.push(json!({
            "event": "harness-event",
            "harness": "gemini",
            "category": "tool-call",
            "toolName": tool_name,
            "input": input,
            "sourceIndex": index,
            "at": Utc::now()
        }));
    }
    events
}

fn normalize_cursor_event(index: usize, value: &Value) -> Vec<Value> {
    let mut event = generic_event("cursor", index, value);
    if let Some(path) = find_path(value) {
        event["category"] = json!("file-change");
        event["path"] = json!(path);
    }
    if let Some(command) = find_command(value) {
        event["category"] = json!("command");
        event["command"] = json!(command);
    }
    vec![event]
}

fn normalize_claudish_event(index: usize, value: &Value) -> Vec<Value> {
    let mut event = generic_event("claudish", index, value);
    event["adapterFallback"] = json!(true);
    vec![event]
}

fn generic_event(harness: &str, index: usize, value: &Value) -> Value {
    let raw_type = raw_event_type(value);
    let lower_type = raw_type.clone().unwrap_or_default().to_ascii_lowercase();
    let text = find_texts(value).into_iter().next();
    let mut category = if lower_type.contains("tool_result")
        || lower_type.contains("tool-result")
        || lower_type.contains("functionresponse")
        || lower_type.contains("function_response")
    {
        "tool-result"
    } else if lower_type.contains("tool")
        || lower_type.contains("functioncall")
        || lower_type.contains("function_call")
    {
        "tool-call"
    } else if lower_type.contains("exec")
        || lower_type.contains("command")
        || find_command(value).is_some()
    {
        "command"
    } else if lower_type.contains("file") || find_path(value).is_some() {
        "file-change"
    } else if find_cost_value(value).is_some() {
        "cost"
    } else if find_session_id(value).is_some() {
        "session"
    } else if text.is_some() {
        "message"
    } else {
        "status"
    };
    if lower_type.contains("error") {
        category = "error";
    }
    let mut record = json!({
        "event": "harness-event",
        "harness": harness,
        "category": category,
        "sourceIndex": index,
        "rawEventType": raw_type,
        "summary": text.as_deref().map(|text| first_chars(text, 240)),
        "raw": value,
        "at": Utc::now()
    });
    if let Some(tool_name) = find_tool_name(value) {
        record["toolName"] = json!(tool_name);
    }
    if let Some(command) = find_command(value) {
        record["command"] = json!(command);
    }
    if let Some(path) = find_path(value) {
        record["path"] = json!(path);
    }
    if let Some(cost) = find_cost_value(value) {
        record["costUsd"] = json!(cost);
    }
    if let Some(session_id) = find_session_id(value) {
        record["sessionId"] = json!(session_id);
    }
    record
}

fn raw_event_type(value: &Value) -> Option<String> {
    for key in ["type", "event", "kind", "subtype", "status"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            return Some(text.to_string());
        }
    }
    value
        .get("item")
        .and_then(raw_event_type)
        .or_else(|| value.get("message").and_then(raw_event_type))
}

fn find_texts(value: &Value) -> Vec<String> {
    let mut out = vec![];
    collect_texts(value, &mut out);
    out.sort();
    out.dedup();
    out
}

fn collect_texts(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if matches!(
                    key.as_str(),
                    "text" | "message" | "summary" | "content" | "delta" | "result"
                ) && let Some(text) = value.as_str()
                    && !text.trim().is_empty()
                {
                    out.push(text.to_string());
                }
                collect_texts(value, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_texts(item, out);
            }
        }
        _ => {}
    }
}

fn find_tool_calls(value: &Value) -> Vec<(String, Value)> {
    let mut out = vec![];
    collect_tool_calls(value, &mut out);
    out
}

fn collect_tool_calls(value: &Value, out: &mut Vec<(String, Value)>) {
    match value {
        Value::Object(map) => {
            let raw_type = raw_event_type(value)
                .unwrap_or_default()
                .to_ascii_lowercase();
            if (raw_type.contains("tool")
                || raw_type.contains("functioncall")
                || raw_type.contains("function_call")
                || map.contains_key("functionCall")
                || map.contains_key("tool_call"))
                && let Some(name) = find_tool_name(value)
            {
                out.push((name, value.clone()));
            }
            for value in map.values() {
                collect_tool_calls(value, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_tool_calls(item, out);
            }
        }
        _ => {}
    }
}

fn find_tool_name(value: &Value) -> Option<String> {
    for key in [
        "toolName",
        "tool_name",
        "name",
        "functionName",
        "function_name",
    ] {
        if let Some(text) = value.get(key).and_then(Value::as_str)
            && !text.trim().is_empty()
        {
            return Some(text.to_string());
        }
    }
    value
        .get("functionCall")
        .and_then(find_tool_name)
        .or_else(|| value.get("tool_call").and_then(find_tool_name))
        .or_else(|| value.get("item").and_then(find_tool_name))
}

fn find_command(value: &Value) -> Option<String> {
    find_string_by_keys(
        value,
        &[
            "command",
            "cmd",
            "shellCommand",
            "shell_command",
            "argv",
            "args",
        ],
    )
}

fn find_path(value: &Value) -> Option<String> {
    find_string_by_keys(
        value,
        &[
            "path",
            "file",
            "filename",
            "filePath",
            "file_path",
            "uri",
            "target",
        ],
    )
}

fn find_string_by_keys(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(value) = map.get(*key) {
                    match value {
                        Value::String(text) if !text.trim().is_empty() => {
                            return Some(text.to_string());
                        }
                        Value::Array(items) if !items.is_empty() => {
                            let joined = items
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(" ");
                            if !joined.trim().is_empty() {
                                return Some(joined);
                            }
                        }
                        _ => {}
                    }
                }
            }
            for value in map.values() {
                if let Some(found) = find_string_by_keys(value, keys) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items
            .iter()
            .find_map(|item| find_string_by_keys(item, keys)),
        _ => None,
    }
}

fn first_chars(text: &str, max_chars: usize) -> String {
    let mut out = String::new();
    for ch in text.chars().take(max_chars) {
        out.push(ch);
    }
    if text.chars().count() > max_chars {
        out.push_str("...");
    }
    out
}

fn extract_cost(structured: &Value, stdout: &str, final_message: &str) -> Option<CostRecord> {
    find_cost_value(structured)
        .map(|usd| CostRecord {
            usd: Some(usd),
            source: "structured-output".to_string(),
            raw: Some(structured.clone()),
        })
        .or_else(|| {
            parse_stdout_values(stdout)
                .into_iter()
                .find_map(|value| find_cost_value(&value).map(|usd| (usd, value)))
                .map(|(usd, raw)| CostRecord {
                    usd: Some(usd),
                    source: "stdout-event".to_string(),
                    raw: Some(raw),
                })
        })
        .or_else(|| {
            extract_json(final_message)
                .and_then(|value| find_cost_value(&value).map(|usd| (usd, value)))
                .map(|(usd, raw)| CostRecord {
                    usd: Some(usd),
                    source: "final-message-json".to_string(),
                    raw: Some(raw),
                })
        })
}

fn find_cost_value(value: &Value) -> Option<f64> {
    for key in [
        "costUsd",
        "cost_usd",
        "totalCostUsd",
        "total_cost_usd",
        "total_cost",
    ] {
        if let Some(cost) = value.get(key).and_then(Value::as_f64) {
            return Some(cost);
        }
    }
    if let Some(usage) = value.get("usage")
        && let Some(cost) = find_cost_value(usage)
    {
        return Some(cost);
    }
    if let Some(result) = value.get("result")
        && let Some(cost) = find_cost_value(result)
    {
        return Some(cost);
    }
    if let Some(usage) = value.get("usageMetadata")
        && let Some(cost) = find_cost_value(usage)
    {
        return Some(cost);
    }
    None
}

fn extract_session_id(structured: &Value, stdout: &str, final_message: &str) -> Option<String> {
    find_session_id(structured)
        .or_else(|| {
            parse_stdout_values(stdout)
                .into_iter()
                .find_map(|value| find_session_id(&value))
        })
        .or_else(|| extract_json(final_message).and_then(|value| find_session_id(&value)))
}

fn find_session_id(value: &Value) -> Option<String> {
    for key in [
        "sessionId",
        "session_id",
        "conversationId",
        "conversation_id",
    ] {
        if let Some(session_id) = value.get(key).and_then(Value::as_str)
            && !session_id.trim().is_empty()
        {
            return Some(session_id.to_string());
        }
    }
    if let Some(result) = value.get("result")
        && let Some(session_id) = find_session_id(result)
    {
        return Some(session_id);
    }
    if let Some(item) = value.get("item")
        && let Some(session_id) = find_session_id(item)
    {
        return Some(session_id);
    }
    None
}

fn select_resume_session(req: &RunRequest, can_resume: bool) -> Result<Option<String>> {
    if !can_resume {
        return Ok(None);
    }
    let _lock = acquire_lock(&req.project_root, "resume")?;
    let map = read_resume_map(&req.project_root)?;
    for key in resume_keys(req) {
        if let Some(session_id) = map
            .get(&key)
            .and_then(|value| value.get("sessionId"))
            .and_then(Value::as_str)
            && !session_id.trim().is_empty()
        {
            return Ok(Some(session_id.to_string()));
        }
    }
    Ok(None)
}

fn record_resume_session(req: &RunRequest, transaction_id: &str, session_id: &str) -> Result<()> {
    let _lock = acquire_lock(&req.project_root, "resume")?;
    let mut map = read_resume_map(&req.project_root)?;
    for key in resume_keys(req) {
        map.insert(
            key,
            json!({
                "sessionId": session_id,
                "transactionId": transaction_id,
                "harness": req.harness,
                "role": req.role,
                "phase": req.phase,
                "pod": req.pod,
                "updatedAt": Utc::now()
            }),
        );
    }
    write_json_pretty(&resume_path(&req.project_root), &map)
}

fn read_resume_map(root: &Path) -> Result<BTreeMap<String, Value>> {
    let path = resume_path(root);
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let data = fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_slice(&data).with_context(|| format!("failed to parse {}", path.display()))
}

fn resume_path(root: &Path) -> PathBuf {
    dialec_dir(root).join("session").join("resume.json")
}

fn resume_keys(req: &RunRequest) -> Vec<String> {
    let scope = req.pod.as_deref().unwrap_or("global");
    vec![
        format!("{}/{}/{}/{}", req.harness, req.role, req.phase, scope),
        format!("{}/{}/{}", req.harness, req.role, req.phase),
        format!("{}/{}", req.harness, req.role),
    ]
}

fn ledger_entries(transaction: &RunTransaction, signal: &ConvergenceSignal) -> Vec<Value> {
    let mut entries: Vec<Value> = signal
        .objections
        .iter()
        .map(|objection| {
            json!({
                "event": "raised",
                "id": objection.id,
                "transactionId": transaction.id,
                "phase": transaction.phase,
                "pod": transaction.pod,
                "role": transaction.role,
                "harness": transaction.harness,
                "category": objection.category,
                "severity": objection.severity,
                "blocking": objection.blocking,
                "status": objection.status,
                "description": objection.description,
                "evidence": objection.evidence,
                "location": objection.location,
                "owner": objection.owner,
                "at": transaction.completed_at,
            })
        })
        .collect();
    for id in &signal.resolved_objection_ids {
        entries.push(json!({
            "event": "resolved",
            "id": id,
            "transactionId": transaction.id,
            "phase": transaction.phase,
            "pod": transaction.pod,
            "role": transaction.role,
            "harness": transaction.harness,
            "status": "addressed",
            "at": transaction.completed_at,
        }));
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_signal_from_wrapped_result_text() {
        let wrapped = json!({
            "type": "result",
            "session_id": "claude-session",
            "result": "{\"verdict\":\"approve\",\"summary\":\"ok\",\"objections\":[],\"resolvedObjectionIds\":[],\"newObjectionIds\":[]}"
        });
        let signal = signal_candidate_from_value(&wrapped).expect("signal");
        assert_eq!(
            signal.get("verdict").and_then(Value::as_str),
            Some("approve")
        );
    }

    #[test]
    fn normalizes_codex_command_event() {
        let stdout = br#"{"type":"exec_command.started","command":"cargo test","session_id":"codex-session"}"#;
        let events = normalize_events("codex", stdout).expect("events");
        let text = String::from_utf8(events).expect("utf8");
        assert!(text.contains("\"category\":\"command\""));
        assert!(text.contains("cargo test"));
        assert!(text.contains("codex-session"));
    }

    #[test]
    fn normalizes_claude_tool_and_cost_events() {
        let stdout = br#"{"type":"assistant","session_id":"claude-session","total_cost_usd":0.12,"message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test"}}]}}"#;
        let events = normalize_events("claude", stdout).expect("events");
        let text = String::from_utf8(events).expect("utf8");
        assert!(text.contains("\"category\":\"tool-call\""));
        assert!(text.contains("\"category\":\"cost\""));
        assert!(text.contains("claude-session"));
    }

    #[test]
    fn resume_keys_do_not_cross_role_boundary() {
        let req = RunRequest {
            phase: "implement".to_string(),
            role: "verifier".to_string(),
            harness: "claude".to_string(),
            task: "verify".to_string(),
            workspace: PathBuf::from("."),
            project_root: PathBuf::from("."),
            pod: Some("auth".to_string()),
            sandbox: "read-only".to_string(),
            approval: "never".to_string(),
            timeout_ms: 1,
            artifacts: vec![],
            max_budget_usd: None,
            max_turns: None,
        };
        assert_eq!(
            resume_keys(&req),
            vec![
                "claude/verifier/implement/auth",
                "claude/verifier/implement",
                "claude/verifier"
            ]
        );
    }
}

struct ExternalOutput {
    exit_code: i32,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    error: Option<HarnessError>,
}
