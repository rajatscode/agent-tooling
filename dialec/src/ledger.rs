use crate::fsutil::dialec_dir;
use crate::model::ConvergenceSignal;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LedgerEntry {
    pub id: String,
    pub event: String,
    pub phase: Option<String>,
    pub pod: Option<String>,
    pub category: Option<String>,
    pub severity: Option<String>,
    pub blocking: Option<bool>,
    pub status: Option<String>,
    pub description: Option<String>,
    pub evidence: Option<String>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone)]
pub struct Ledger {
    entries: Vec<LedgerEntry>,
}

impl Ledger {
    pub fn read(root: &Path) -> Result<Self> {
        let path = dialec_dir(root).join("session").join("objections.jsonl");
        if !path.exists() {
            return Ok(Self { entries: vec![] });
        }
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let entries = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str::<LedgerEntry>(line).ok())
            .collect();
        Ok(Self { entries })
    }

    pub fn open_blocking(&self, phase: &str, pod: Option<&str>) -> Vec<LedgerEntry> {
        let mut open = BTreeMap::<String, LedgerEntry>::new();
        let mut closed = BTreeSet::<String>::new();

        for entry in &self.entries {
            if !matches_scope(entry, phase, pod) {
                continue;
            }
            if entry.event == "resolved"
                || matches!(
                    entry.status.as_deref(),
                    Some("addressed" | "withdrawn" | "user-accepted")
                )
            {
                closed.insert(entry.id.clone());
                open.remove(&entry.id);
                continue;
            }
            let is_blocking = entry.blocking.unwrap_or(false)
                || matches!(entry.severity.as_deref(), Some("blocker" | "major"));
            if is_blocking && !closed.contains(&entry.id) {
                open.insert(entry.id.clone(), entry.clone());
            }
        }

        open.into_values().collect()
    }
}

pub fn signal_has_blockers(signal: &ConvergenceSignal) -> bool {
    signal.objections.iter().any(|objection| {
        objection.status == "open"
            && (objection.blocking || matches!(objection.severity.as_str(), "blocker" | "major"))
    })
}

pub fn signal_converged(signal: &ConvergenceSignal) -> bool {
    matches!(signal.verdict.as_str(), "approve" | "approve-with-nits")
        && !signal_has_blockers(signal)
}

fn matches_scope(entry: &LedgerEntry, phase: &str, pod: Option<&str>) -> bool {
    if entry.phase.as_deref() != Some(phase) {
        return false;
    }
    match pod {
        Some(pod) => entry.pod.as_deref() == Some(pod),
        None => true,
    }
}
