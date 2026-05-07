use crate::fsutil::dialec_dir;
use crate::ledger::Ledger;
use crate::model::DialecState;
use anyhow::Result;
use serde_json::{json, Value};
use std::fs;
use std::path::Path;

/// Agent-readable convergence status.
/// Write to .dialec/status.json so agents can query "am I converged?" without scanning ledger.
pub fn write_status(root: &Path, state: &DialecState) -> Result<()> {
    let ledger = Ledger::read(root)?;

    let phase = state.current_phase.as_str();
    let blockers = ledger.open_blocking(phase, None);

    let status = json!({
        "phase": phase,
        "converged": blockers.is_empty(),
        "blockers": blockers.len(),
        "blocking_objections": blockers.iter().map(|e| {
            json!({
                "id": e.id,
                "category": e.category,
                "severity": e.severity,
                "description": e.description,
            })
        }).collect::<Vec<_>>(),
        "at": chrono::Utc::now().to_rfc3339(),
    });

    let path = dialec_dir(root).join("status.json");
    fs::write(&path, serde_json::to_string_pretty(&status)?)?;
    Ok(())
}

/// Agent queries: is phase X converged?
/// Returns true if no open blockers in that phase.
pub fn is_converged(root: &Path, phase: &str) -> Result<bool> {
    let ledger = Ledger::read(root)?;
    let blockers = ledger.open_blocking(phase, None);
    Ok(blockers.is_empty())
}

/// Get human-readable reason why phase is blocked (for agent feedback).
pub fn blocker_summary(root: &Path, phase: &str) -> Result<String> {
    let ledger = Ledger::read(root)?;
    let blockers = ledger.open_blocking(phase, None);

    if blockers.is_empty() {
        return Ok("No blockers".to_string());
    }

    let summaries: Vec<String> = blockers.iter()
        .map(|e| format!("[{}] {}", e.severity.as_deref().unwrap_or("?"),
                         e.description.as_deref().unwrap_or("(no description)")))
        .collect();

    Ok(format!("Blocked on:\n- {}", summaries.join("\n- ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blocker_summary() {
        // Would need a test ledger
    }
}
