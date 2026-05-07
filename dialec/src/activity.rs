/// Activity log queries for coordinator to use instead of ledger scans.
use crate::fsutil::dialec_dir;
use crate::model::ConvergenceSignal;
use anyhow::Result;
use serde_json::Value;
use std::fs;
use std::path::Path;

/// Get the most recent convergence-check event for a phase.
pub fn latest_convergence_check(root: &Path, phase: &str) -> Result<Option<Value>> {
    let path = dialec_dir(root).join("log").join("activity.jsonl");
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)?;
    let mut latest = None;

    for line in content.lines().rev() {
        if let Ok(event) = serde_json::from_str::<Value>(line) {
            if event.get("event").and_then(Value::as_str) == Some("convergence-check") {
                if event.get("phase").and_then(Value::as_str) == Some(phase) {
                    latest = Some(event);
                    break;
                }
            }
        }
    }

    Ok(latest)
}

/// Did phase converge? (query activity log)
pub fn is_phase_converged(root: &Path, phase: &str) -> Result<bool> {
    match latest_convergence_check(root, phase)? {
        Some(event) => Ok(event
            .get("converged")
            .and_then(Value::as_bool)
            .unwrap_or(false)),
        None => Ok(false),
    }
}

/// Get blocker count for phase (query activity log)
pub fn phase_blocker_count(root: &Path, phase: &str) -> Result<u32> {
    match latest_convergence_check(root, phase)? {
        Some(event) => Ok(event
            .get("blockers")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32),
        None => Ok(0),
    }
}

/// Get the latest signal-parsed event
pub fn latest_signal(root: &Path) -> Result<Option<ConvergenceSignal>> {
    let path = dialec_dir(root).join("log").join("activity.jsonl");
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(&path)?;

    for line in content.lines().rev() {
        if let Ok(event) = serde_json::from_str::<Value>(line) {
            if event.get("event").and_then(Value::as_str) == Some("signal-parsed") {
                // Parse verdict from the event
                if let Some(verdict) = event.get("verdict").and_then(Value::as_str) {
                    return Ok(Some(ConvergenceSignal {
                        verdict: verdict.to_string(),
                        summary: event
                            .get("summary")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        objections: vec![],
                        resolved_objection_ids: vec![],
                        new_objection_ids: vec![],
                    }));
                }
            }
        }
    }

    Ok(None)
}

/// Has an agent completed? (check activity log for agent-complete)
pub fn has_agent_completed(root: &Path, turn_id: &str) -> Result<bool> {
    let path = dialec_dir(root).join("log").join("activity.jsonl");
    if !path.exists() {
        return Ok(false);
    }

    let content = fs::read_to_string(&path)?;

    for line in content.lines() {
        if let Ok(event) = serde_json::from_str::<Value>(line) {
            if event.get("event").and_then(Value::as_str) == Some("agent-complete") {
                if event.get("turn_id").and_then(Value::as_str) == Some(turn_id) {
                    return Ok(true);
                }
            }
        }
    }

    Ok(false)
}
