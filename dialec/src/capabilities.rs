use crate::config::load_or_default;
use crate::fsutil::{dialec_dir, write_json_pretty};
use crate::model::{HarnessCapabilities, PromptInjectionCapability, StructuredOutputCapability};
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn probe_all(root: &Path, write: bool) -> Result<BTreeMap<String, HarnessCapabilities>> {
    let config = load_or_default(root)?;
    let mut reports = BTreeMap::new();
    for name in config.harnesses.keys() {
        let report = probe_harness(name, root)?;
        if write {
            let path = dialec_dir(root)
                .join("capabilities")
                .join(format!("{name}.json"));
            write_json_pretty(&path, &report)?;
        }
        reports.insert(name.clone(), report);
    }
    Ok(reports)
}

pub fn probe_harness(name: &str, root: &Path) -> Result<HarnessCapabilities> {
    let config = load_or_default(root)?;
    let Some(harness) = config.harnesses.get(name) else {
        anyhow::bail!("unknown harness: {name}");
    };

    let command_paths = harness
        .command_candidates
        .iter()
        .flat_map(|candidate| resolve_commands(candidate))
        .fold(Vec::<PathBuf>::new(), |mut acc, path| {
            if !acc.contains(&path) {
                acc.push(path);
            }
            acc
        });

    if command_paths.is_empty() {
        return Ok(unavailable_report(
            name,
            &format!(
                "none of these commands were found on PATH: {}",
                harness.command_candidates.join(", ")
            ),
        ));
    }

    let mut reports = Vec::new();
    for path in command_paths {
        match probe_command(name, root, &path) {
            Ok(report) => reports.push(report),
            Err(error) => reports.push(unavailable_report(
                name,
                &format!("{} failed to probe: {error}", path.display()),
            )),
        }
    }

    let mut iter = reports.into_iter();
    let mut best = iter
        .next()
        .expect("reports cannot be empty after command_paths check");
    for other in iter {
        if report_score(&other) > report_score(&best) {
            if let Some(command) = best.command {
                let mut promoted = other;
                promoted
                    .limitations
                    .push(format!("also found lower-scoring candidate: {command}"));
                best = promoted;
            } else {
                best = other;
            }
        } else if let Some(command) = other.command {
            best.limitations
                .push(format!("also found lower-scoring candidate: {command}"));
        }
    }
    Ok(best)
}

fn probe_command(name: &str, root: &Path, command_path: &Path) -> Result<HarnessCapabilities> {
    let command = command_path.to_string_lossy().to_string();
    let version = command_output(&command, &["--version"], root).ok();
    let help = command_output(&command, &["--help"], root).unwrap_or_default();
    let extra_help = match name {
        "codex" => command_output(&command, &["exec", "--help"], root).unwrap_or_default(),
        _ => String::new(),
    };
    let combined_help = format!("{help}\n{extra_help}");

    let mut limitations = vec![];
    let mut prompt_injection = vec![];
    let mut sandbox_modes = vec![];
    let mut approval_modes = vec![];
    let mut output_modes = vec![];
    let mut structured_output = StructuredOutputCapability {
        supported: false,
        mechanism: None,
        schema_flag: None,
    };

    match name {
        "claude" => {
            output_modes.extend(modes_from_help(&combined_help));
            structured_output = StructuredOutputCapability {
                supported: combined_help.contains("--json-schema"),
                mechanism: Some("json-schema flag".to_string()),
                schema_flag: Some("--json-schema".to_string()),
            };
            if combined_help.contains("--append-system-prompt-file") {
                prompt_injection.push(PromptInjectionCapability {
                    mechanism: "system-prompt-file".to_string(),
                    flag: Some("--append-system-prompt-file".to_string()),
                    file_name: None,
                    probed: true,
                });
            }
            if combined_help.contains("--system-prompt") {
                prompt_injection.push(PromptInjectionCapability {
                    mechanism: "system-prompt".to_string(),
                    flag: Some("--system-prompt".to_string()),
                    file_name: None,
                    probed: true,
                });
            }
            approval_modes.extend([
                "default".to_string(),
                "dontAsk".to_string(),
                "bypassPermissions".to_string(),
            ]);
            if combined_help.contains("--permission-mode") {
                limitations.push(
                    "Claude exposes permission modes, not Codex-style sandbox modes.".to_string(),
                );
            }
        }
        "codex" => {
            output_modes.push("jsonl".to_string());
            structured_output = StructuredOutputCapability {
                supported: combined_help.contains("--output-schema"),
                mechanism: Some("output-schema flag".to_string()),
                schema_flag: Some("--output-schema".to_string()),
            };
            sandbox_modes.extend([
                "read-only".to_string(),
                "workspace-write".to_string(),
                "danger-full-access".to_string(),
            ]);
            approval_modes.extend([
                "untrusted".to_string(),
                "on-request".to_string(),
                "never".to_string(),
            ]);
            let agents_probe = probe_codex_agents_file(&command).unwrap_or(false);
            prompt_injection.push(PromptInjectionCapability {
                mechanism: "project-instructions-file".to_string(),
                flag: None,
                file_name: Some("AGENTS.md".to_string()),
                probed: agents_probe,
            });
            limitations.push(
                "Codex --json emits event JSONL on stdout; -o writes only the last message."
                    .to_string(),
            );
        }
        "gemini" => {
            output_modes.extend(modes_from_help(&combined_help));
            prompt_injection.push(PromptInjectionCapability {
                mechanism: "project-instructions-file-or-prompt-prefix".to_string(),
                flag: None,
                file_name: Some("GEMINI.md".to_string()),
                probed: combined_help.contains("GEMINI.md"),
            });
            approval_modes.push("yolo".to_string());
            structured_output = StructuredOutputCapability {
                supported: combined_help.contains("--output-format")
                    && combined_help.contains("json"),
                mechanism: Some("json output; no probed schema flag".to_string()),
                schema_flag: None,
            };
        }
        "cursor" => {
            output_modes.extend(modes_from_help(&combined_help));
            prompt_injection.push(PromptInjectionCapability {
                mechanism: "cursor-rules-or-prompt-prefix".to_string(),
                flag: None,
                file_name: Some(".cursor/rules".to_string()),
                probed: combined_help.contains("rules") || combined_help.contains(".cursor"),
            });
            approval_modes.push("force".to_string());
            structured_output = StructuredOutputCapability {
                supported: combined_help.contains("--output-format")
                    && combined_help.contains("json"),
                mechanism: Some("json output; no probed schema flag".to_string()),
                schema_flag: None,
            };
            limitations.push(
                "Cursor adapter must use cursor-agent, not any local agent helper.".to_string(),
            );
        }
        _ => limitations.push("unknown harness type; generic probing only".to_string()),
    }

    output_modes.sort();
    output_modes.dedup();

    Ok(HarnessCapabilities {
        name: name.to_string(),
        available: true,
        command: Some(command),
        version,
        authed: auth_hint(name),
        cwd_flag: cwd_flag(name, &combined_help),
        headless: headless_hint(name, &combined_help),
        output_modes,
        structured_output,
        prompt_injection,
        sandbox_modes,
        approval_modes,
        can_resume: can_resume(name, &combined_help),
        can_report_cost: combined_help.contains("--max-budget-usd"),
        can_stream_events: combined_help.contains("stream-json")
            || combined_help.contains("--json"),
        can_emit_tool_events: combined_help.contains("stream-json") || name == "codex",
        supports_extra_writable_roots: combined_help.contains("--add-dir"),
        limitations,
    })
}

fn report_score(report: &HarnessCapabilities) -> u8 {
    let mut score = 0;
    if report.available {
        score += 1;
    }
    if report.headless {
        score += 2;
    }
    if report.structured_output.supported {
        score += 4;
    }
    if report.can_stream_events {
        score += 1;
    }
    score
}

fn unavailable_report(name: &str, reason: &str) -> HarnessCapabilities {
    HarnessCapabilities {
        name: name.to_string(),
        available: false,
        command: None,
        version: None,
        authed: None,
        cwd_flag: None,
        headless: false,
        output_modes: vec![],
        structured_output: StructuredOutputCapability {
            supported: false,
            mechanism: None,
            schema_flag: None,
        },
        prompt_injection: vec![],
        sandbox_modes: vec![],
        approval_modes: vec![],
        can_resume: false,
        can_report_cost: false,
        can_stream_events: false,
        can_emit_tool_events: false,
        supports_extra_writable_roots: false,
        limitations: vec![reason.to_string()],
    }
}

fn modes_from_help(help: &str) -> Vec<String> {
    let mut modes = vec![];
    if help.contains("text") {
        modes.push("text".to_string());
    }
    if help.contains("json") {
        modes.push("json".to_string());
    }
    if help.contains("stream-json") {
        modes.push("stream-json".to_string());
    }
    modes
}

fn cwd_flag(name: &str, help: &str) -> Option<String> {
    if help.contains("--cd") {
        Some("--cd".to_string())
    } else if name == "claude" && help.contains("--add-dir") {
        Some("cwd + --add-dir".to_string())
    } else if help.contains("--include-directories") {
        Some("--include-directories".to_string())
    } else {
        Some("process cwd".to_string())
    }
}

fn headless_hint(name: &str, help: &str) -> bool {
    match name {
        "codex" => help.contains("Run Codex non-interactively") || help.contains("exec"),
        _ => help.contains("-p") || help.contains("--print") || help.contains("--output-format"),
    }
}

fn can_resume(name: &str, help: &str) -> bool {
    match name {
        "codex" => help.contains("resume"),
        _ => help.contains("--resume") || help.contains("resume"),
    }
}

fn command_output(command: &str, args: &[&str], cwd: &Path) -> Result<String> {
    let output = Command::new(command)
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to run {command} {}", args.join(" ")))?;
    let mut text = String::new();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    if !output.status.success() && text.trim().is_empty() {
        anyhow::bail!("{command} {} failed", args.join(" "));
    }
    Ok(text.trim().to_string())
}

fn resolve_commands(candidate: &str) -> Vec<PathBuf> {
    let candidate_path = Path::new(candidate);
    if candidate_path.components().count() > 1 {
        return if candidate_path.exists() {
            vec![candidate_path.to_path_buf()]
        } else {
            vec![]
        };
    }

    let Some(path) = env::var_os("PATH") else {
        return vec![];
    };
    env::split_paths(&path)
        .map(|dir| dir.join(candidate))
        .filter(|path| path.is_file())
        .collect()
}

fn auth_hint(name: &str) -> Option<bool> {
    let home = env::var_os("HOME").map(PathBuf::from)?;
    match name {
        "codex" => Some(
            env::var_os("OPENAI_API_KEY").is_some()
                || env::var_os("CODEX_HOME").is_some()
                || home.join(".codex").join("auth.json").exists(),
        ),
        "claude" => Some(
            env::var_os("ANTHROPIC_API_KEY").is_some()
                || home.join(".claude").exists()
                || home.join(".config").join("claude").exists(),
        ),
        "gemini" => Some(env::var_os("GEMINI_API_KEY").is_some() || home.join(".gemini").exists()),
        "cursor" => Some(home.join(".cursor").exists()),
        _ => None,
    }
}

fn probe_codex_agents_file(command: &str) -> Result<bool> {
    let temp = tempfile::tempdir().context("failed to create temporary Codex probe dir")?;
    fs::write(
        temp.path().join("AGENTS.md"),
        "dialec AGENTS probe marker\n",
    )
    .context("failed to write AGENTS.md probe")?;
    let _ = Command::new("git")
        .arg("init")
        .arg("-q")
        .current_dir(temp.path())
        .output();
    let output = Command::new(command)
        .args(["debug", "prompt-input", "dialec probe"])
        .current_dir(temp.path())
        .output()
        .context("failed to run codex debug prompt-input")?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(combined.contains("dialec AGENTS probe marker"))
}
