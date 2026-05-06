use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessCapabilities {
    pub name: String,
    pub available: bool,
    pub command: Option<String>,
    pub version: Option<String>,
    pub authed: Option<bool>,
    pub cwd_flag: Option<String>,
    pub headless: bool,
    pub output_modes: Vec<String>,
    pub structured_output: StructuredOutputCapability,
    pub prompt_injection: Vec<PromptInjectionCapability>,
    pub sandbox_modes: Vec<String>,
    pub approval_modes: Vec<String>,
    pub can_resume: bool,
    pub can_report_cost: bool,
    pub can_stream_events: bool,
    pub can_emit_tool_events: bool,
    pub supports_extra_writable_roots: bool,
    pub limitations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StructuredOutputCapability {
    pub supported: bool,
    pub mechanism: Option<String>,
    pub schema_flag: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptInjectionCapability {
    pub mechanism: String,
    pub flag: Option<String>,
    pub file_name: Option<String>,
    pub probed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactRef {
    pub id: String,
    pub path: String,
    pub sha256: String,
    pub artifact_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvergenceSignal {
    pub verdict: String,
    pub summary: String,
    pub objections: Vec<Objection>,
    pub resolved_objection_ids: Vec<String>,
    pub new_objection_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Objection {
    pub id: String,
    pub category: String,
    pub severity: String,
    pub description: String,
    pub blocking: bool,
    pub evidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub proposed_resolution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSnapshot {
    pub path: String,
    pub git_root: Option<String>,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub status: Vec<String>,
    pub dirty: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandInvocation {
    pub argv: Vec<String>,
    pub cwd: String,
    pub timeout_ms: u64,
    pub env_allowlist: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessError {
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunTransaction {
    pub id: String,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pod: Option<String>,
    pub role: String,
    pub harness: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness_version: Option<String>,
    pub workspace: String,
    pub started_at: DateTime<Utc>,
    pub completed_at: DateTime<Utc>,
    pub command: CommandInvocation,
    pub input_artifacts: Vec<ArtifactRef>,
    pub before: WorkspaceSnapshot,
    pub after: WorkspaceSnapshot,
    pub stdout: ArtifactRef,
    pub stderr: ArtifactRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_log: Option<ArtifactRef>,
    pub final_message: ArtifactRef,
    pub structured: ArtifactRef,
    pub signal: ConvergenceSignal,
    pub patch: ArtifactRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<CostRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub exit_code: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<HarnessError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    pub harnesses: BTreeMap<String, HarnessConfig>,
    pub roles: BTreeMap<String, String>,
    pub convergence: ConvergenceConfig,
    pub workspaces: WorkspaceConfig,
    pub budget: BudgetConfig,
    #[serde(default)]
    pub reminders: ReminderConfig,
    #[serde(default)]
    pub workflows: BTreeMap<String, Workflow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessConfig {
    pub command_candidates: Vec<String>,
    pub probe: Vec<String>,
    pub defaults: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvergenceConfig {
    pub max_rounds: u32,
    pub use_arbiter: bool,
    pub arbiter_model: String,
    pub auto_advance_on_nits: bool,
    pub fail_closed_categories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceConfig {
    pub strategy: String,
    pub base_branch: String,
    pub keep_failed_workspaces: bool,
    pub dirty_user_workspace_policy: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetConfig {
    pub max_usd: f64,
    pub per_turn_max_usd: f64,
    pub per_phase_max_usd: Option<f64>,
    pub max_hours: Option<f64>,
    pub max_turns: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CostRecord {
    pub usd: Option<f64>,
    pub source: String,
    pub raw: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReminderConfig {
    pub enabled: bool,
    pub every_turns: u32,
    pub global_rules: Vec<String>,
    pub role_rules: BTreeMap<String, Vec<String>>,
}

impl Default for ReminderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            every_turns: 1,
            global_rules: vec![],
            role_rules: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub description: Option<String>,
    pub phases: Vec<WorkflowPhase>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowPhase {
    pub name: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub steps: Vec<WorkflowStep>,
    #[serde(default)]
    pub repeat_until_converged: bool,
    pub max_rounds: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowStep {
    pub role: String,
    pub task: String,
    pub harness: Option<String>,
    pub workspace: Option<PathBuf>,
    #[serde(default = "default_workflow_sandbox")]
    pub sandbox: String,
    #[serde(default = "default_workflow_approval")]
    pub approval: String,
    #[serde(default)]
    pub artifacts: Vec<PathBuf>,
}

fn default_workflow_sandbox() -> String {
    "read-only".to_string()
}

fn default_workflow_approval() -> String {
    "never".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialecState {
    pub session_id: String,
    pub mode: String,
    pub started_at: DateTime<Utc>,
    pub current_phase: String,
    pub goal: Option<String>,
    pub phases: BTreeMap<String, Value>,
    pub total_cost: f64,
    pub budget: BudgetConfig,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coordinator: Option<CoordinatorState>,
    #[serde(default)]
    pub total_turns: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoordinatorState {
    pub pid: u32,
    pub started_at: DateTime<Utc>,
    pub stdout: String,
    pub stderr: String,
    pub command: Vec<String>,
    pub status: String,
}

#[derive(Debug, Clone)]
pub struct RunRequest {
    pub phase: String,
    pub role: String,
    pub harness: String,
    pub task: String,
    pub workspace: PathBuf,
    pub project_root: PathBuf,
    pub pod: Option<String>,
    pub sandbox: String,
    pub approval: String,
    pub timeout_ms: u64,
    pub artifacts: Vec<PathBuf>,
    pub max_budget_usd: Option<f64>,
    pub max_turns: Option<u32>,
}
