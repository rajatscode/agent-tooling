use crate::model::{ConvergenceSignal, Objection};
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

pub fn signal_schema() -> Value {
    json!({
      "$schema": "http://json-schema.org/draft-07/schema#",
      "type": "object",
      "additionalProperties": false,
      "required": [
        "verdict",
        "summary",
        "objections",
        "resolvedObjectionIds",
        "newObjectionIds"
      ],
      "properties": {
        "verdict": {
          "type": "string",
          "enum": ["approve", "reject", "approve-with-nits"]
        },
        "summary": { "type": "string" },
        "resolvedObjectionIds": {
          "type": "array",
          "items": { "type": "string" }
        },
        "newObjectionIds": {
          "type": "array",
          "items": { "type": "string" }
        },
        "objections": {
          "type": "array",
          "items": {
            "type": "object",
            "additionalProperties": false,
            "required": [
              "id",
              "category",
              "severity",
              "description",
              "blocking",
              "evidence",
              "proposedResolution",
              "location",
              "owner",
              "status"
            ],
            "properties": {
              "id": { "type": "string" },
              "category": {
                "type": "string",
                "enum": [
                  "correctness",
                  "completeness",
                  "clarity",
                  "architecture",
                  "intent-mismatch",
                  "style",
                  "security",
                  "test-coverage",
                  "operability",
                  "performance"
                ]
              },
              "severity": {
                "type": "string",
                "enum": ["blocker", "major", "minor", "nit"]
              },
              "description": { "type": "string" },
              "blocking": { "type": "boolean" },
              "evidence": { "type": "string" },
              "proposedResolution": { "type": ["string", "null"] },
              "location": { "type": ["string", "null"] },
              "owner": { "type": ["string", "null"] },
              "status": {
                "type": "string",
                "enum": ["open", "addressed", "withdrawn", "user-accepted"]
              }
            }
          }
        }
      }
    })
}

pub fn parse_signal(value: &Value) -> Result<ConvergenceSignal> {
    let signal: ConvergenceSignal = serde_json::from_value(value.clone())
        .context("structured output is not a convergence signal")?;
    validate_signal(&signal)?;
    Ok(signal)
}

pub fn validate_signal(signal: &ConvergenceSignal) -> Result<()> {
    match signal.verdict.as_str() {
        "approve" | "reject" | "approve-with-nits" => {}
        other => return Err(anyhow!("invalid verdict: {other}")),
    }

    for objection in &signal.objections {
        validate_objection(objection)?;
    }

    Ok(())
}

fn validate_objection(objection: &Objection) -> Result<()> {
    if objection.id.trim().is_empty() {
        return Err(anyhow!("objection id is empty"));
    }
    if objection.description.trim().is_empty() {
        return Err(anyhow!("objection {} has empty description", objection.id));
    }
    if objection.evidence.trim().is_empty() {
        return Err(anyhow!("objection {} has empty evidence", objection.id));
    }

    let valid_category = matches!(
        objection.category.as_str(),
        "correctness"
            | "completeness"
            | "clarity"
            | "architecture"
            | "intent-mismatch"
            | "style"
            | "security"
            | "test-coverage"
            | "operability"
            | "performance"
    );
    if !valid_category {
        return Err(anyhow!(
            "objection {} has invalid category {}",
            objection.id,
            objection.category
        ));
    }

    let valid_severity = matches!(
        objection.severity.as_str(),
        "blocker" | "major" | "minor" | "nit"
    );
    if !valid_severity {
        return Err(anyhow!(
            "objection {} has invalid severity {}",
            objection.id,
            objection.severity
        ));
    }

    let valid_status = matches!(
        objection.status.as_str(),
        "open" | "addressed" | "withdrawn" | "user-accepted"
    );
    if !valid_status {
        return Err(anyhow!(
            "objection {} has invalid status {}",
            objection.id,
            objection.status
        ));
    }

    Ok(())
}

pub fn fallback_reject_signal(id: &str, message: impl Into<String>) -> ConvergenceSignal {
    let message = message.into();
    ConvergenceSignal {
        verdict: "reject".to_string(),
        summary: message.clone(),
        objections: vec![Objection {
            id: id.to_string(),
            category: "operability".to_string(),
            severity: "blocker".to_string(),
            description: message,
            blocking: true,
            evidence:
                "Dialec could not parse a valid structured convergence signal for this transaction."
                    .to_string(),
            proposed_resolution: Some(
                "Fix the harness output or rerun with a valid signal schema.".to_string(),
            ),
            location: None,
            owner: Some("dialec".to_string()),
            status: "open".to_string(),
        }],
        resolved_objection_ids: vec![],
        new_objection_ids: vec![id.to_string()],
    }
}

pub fn extract_json(text: &str) -> Option<Value> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Ok(value) = serde_json::from_str::<Value>(trimmed) {
        return Some(value);
    }

    if let Some(value) = extract_fenced_json(trimmed) {
        return Some(value);
    }

    extract_embedded_object(trimmed)
}

fn extract_fenced_json(text: &str) -> Option<Value> {
    let mut rest = text;
    while let Some(start) = rest.find("```") {
        rest = &rest[start + 3..];
        let after_lang = rest.strip_prefix("json").unwrap_or(rest);
        let after_newline = after_lang.strip_prefix('\n').unwrap_or(after_lang);
        if let Some(end) = after_newline.find("```") {
            let candidate = &after_newline[..end];
            if let Ok(value) = serde_json::from_str::<Value>(candidate.trim()) {
                return Some(value);
            }
            rest = &after_newline[end + 3..];
        } else {
            break;
        }
    }
    None
}

fn extract_embedded_object(text: &str) -> Option<Value> {
    let starts: Vec<_> = text.match_indices('{').map(|(idx, _)| idx).collect();
    let ends: Vec<_> = text.match_indices('}').map(|(idx, _)| idx + 1).collect();
    for start in starts {
        for end in ends.iter().rev().copied() {
            if end <= start {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<Value>(&text[start..end]) {
                return Some(value);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_fenced_json() {
        let value = extract_json("ok\n```json\n{\"verdict\":\"approve\"}\n```").unwrap();
        assert_eq!(value["verdict"], "approve");
    }

    #[test]
    fn rejects_bad_verdict() {
        let signal = ConvergenceSignal {
            verdict: "maybe".to_string(),
            summary: "x".to_string(),
            objections: vec![],
            resolved_objection_ids: vec![],
            new_objection_ids: vec![],
        };
        assert!(validate_signal(&signal).is_err());
    }
}
