use crate::fsutil::{append_line, dialec_dir, ensure_dir};
use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub from: String,
    pub to: String,
    pub kind: MessageKind,
    pub body: String,
    pub at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessageKind {
    Directive,
    Question,
    Update,
    Cancel,
    Nudge,
}

impl std::fmt::Display for MessageKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageKind::Directive => write!(f, "directive"),
            MessageKind::Question => write!(f, "question"),
            MessageKind::Update => write!(f, "update"),
            MessageKind::Cancel => write!(f, "cancel"),
            MessageKind::Nudge => write!(f, "nudge"),
        }
    }
}

/// Get the channel directory for a given target (role, pod, or turn id)
pub fn channel_dir(root: &Path, target: &str) -> PathBuf {
    dialec_dir(root).join("channels").join(target)
}

/// Send a message from parent to a target channel
pub fn send_message(
    root: &Path,
    from: &str,
    to: &str,
    kind: MessageKind,
    body: &str,
    context: Option<Value>,
) -> Result<Message> {
    let dir = channel_dir(root, to);
    ensure_dir(&dir)?;
    let inbox = dir.join("inbox.jsonl");
    let msg = Message {
        id: uuid::Uuid::new_v4().to_string(),
        from: from.to_string(),
        to: to.to_string(),
        kind,
        body: body.to_string(),
        at: Utc::now().to_rfc3339(),
        context,
    };
    append_line(&inbox, &serde_json::to_string(&msg)?)?;
    Ok(msg)
}

/// Read all unread messages for a target (returns all messages in inbox)
pub fn read_inbox(root: &Path, target: &str) -> Result<Vec<Message>> {
    let inbox = channel_dir(root, target).join("inbox.jsonl");
    if !inbox.exists() {
        return Ok(vec![]);
    }
    let content = fs::read_to_string(&inbox)
        .with_context(|| format!("failed to read {}", inbox.display()))?;
    Ok(content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

/// Read messages since a given message id (for incremental reads)
pub fn read_inbox_since(root: &Path, target: &str, since_id: Option<&str>) -> Result<Vec<Message>> {
    let messages = read_inbox(root, target)?;
    match since_id {
        None => Ok(messages),
        Some(id) => {
            let pos = messages.iter().position(|m| m.id == id);
            match pos {
                Some(idx) => Ok(messages[idx + 1..].to_vec()),
                None => Ok(messages),
            }
        }
    }
}

/// Format inbox messages for injection into an agent's prompt context
pub fn format_inbox_for_prompt(messages: &[Message]) -> String {
    if messages.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str("## Messages From Coordinator\n\n");
    out.push_str("The following messages were sent to you during this session. Read and act on them.\n\n");
    for msg in messages {
        out.push_str(&format!(
            "### [{kind}] from `{from}` at {at}\n\n{body}\n\n",
            kind = msg.kind,
            from = msg.from,
            at = msg.at,
            body = msg.body,
        ));
    }
    out
}

/// List all active channels
pub fn list_channels(root: &Path) -> Result<Vec<String>> {
    let dir = dialec_dir(root).join("channels");
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut channels = vec![];
    for entry in fs::read_dir(&dir)? {
        let entry = entry?;
        if entry.path().is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                channels.push(name.to_string());
            }
        }
    }
    channels.sort();
    Ok(channels)
}

/// Generate the channel instruction block for agent prompts
pub fn channel_instructions(root: &Path, role: &str) -> String {
    let channel_path = channel_dir(root, role).join("inbox.jsonl");
    format!(
        r#"## Inter-Agent Communication

You have a message channel. Check `{}` for messages from the coordinator or other agents. Messages are JSONL with fields: id, from, to, kind (directive/question/update/cancel/nudge), body, at.

If you receive a `directive`, follow it. If you receive a `question`, answer it in your output. If you receive a `cancel`, stop your current work and produce a convergence signal immediately. If you receive a `nudge`, acknowledge and continue.

To send a message back, include it in your convergence signal summary or write to `.dialec/channels/coordinator/inbox.jsonl`.
"#,
        channel_path.display()
    )
}
