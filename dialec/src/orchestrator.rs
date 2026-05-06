use crate::config::load_or_default;
use crate::fsutil::{dialec_dir, ensure_dir, write_json_pretty};
use crate::git;
use crate::ledger::{Ledger, signal_converged};
use crate::model::{Config, DialecState, RunRequest, RunTransaction};
use crate::session::{log_cost, log_decision, log_timeline, read_state, sanitize, write_state};
use crate::transaction::run_transaction;
use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

#[derive(Debug, Clone)]
pub struct DriveOptions {
    pub max_rounds: Option<u32>,
    pub max_parallel: usize,
    pub phase: Option<String>,
    pub no_cleanup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PodPlan {
    pods: Vec<Pod>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pod {
    name: String,
    spec_slice: String,
}

#[derive(Debug, Clone)]
struct PodResult {
    name: String,
    branch: String,
    worktree_name: String,
}

pub fn drive(root: &Path, options: DriveOptions) -> Result<()> {
    let mut state =
        read_state(root).context("no Dialec session found; run `dialec start` first")?;
    let config = load_or_default(root)?;
    let configured_rounds = options.max_rounds.unwrap_or(config.convergence.max_rounds);
    let max_rounds = if state.mode == "autonomous" {
        configured_rounds.min(3)
    } else {
        configured_rounds
    };
    if options.max_parallel > 1 {
        log_decision(
            root,
            json!({
                "event": "parallel-requested",
                "maxParallel": options.max_parallel,
                "decision": "pods run concurrently in isolated worktrees; converged branches merge through an ordered queue",
                "at": Utc::now()
            }),
        )?;
    }

    loop {
        let phase = options
            .phase
            .clone()
            .unwrap_or_else(|| state.current_phase.clone());
        match phase.as_str() {
            "spec" => {
                drive_spec(root, &config, &mut state, max_rounds)?;
                if options.phase.is_some() {
                    break;
                }
            }
            "implement" => {
                drive_implementation(root, &config, &mut state, max_rounds, options.max_parallel)?;
                if options.phase.is_some() {
                    break;
                }
            }
            "cleanup" => {
                if options.no_cleanup {
                    mark_phase(&mut state, "cleanup", "skipped");
                    write_state(root, &state)?;
                    break;
                }
                drive_cleanup(root, &config, &mut state, max_rounds)?;
                break;
            }
            "done" => break,
            other => return Err(anyhow!("unknown phase: {other}")),
        }
        if state.current_phase == "done" {
            break;
        }
    }
    Ok(())
}

fn drive_spec(
    root: &Path,
    config: &Config,
    state: &mut DialecState,
    max_rounds: u32,
) -> Result<()> {
    let phase_dir = dialec_dir(root).join("session").join("phase-spec");
    ensure_dir(&phase_dir)?;
    let goal = state.goal.clone().unwrap_or_else(|| {
        fs::read_to_string(phase_dir.join("goal.md"))
            .unwrap_or_else(|_| "No goal provided.".to_string())
    });

    for round in 1..=max_rounds {
        let draft = phase_dir.join(format!("draft-{round}.md"));
        if !draft.exists() {
            let tx = run_role(
                root,
                config,
                RoleRun {
                    phase: "spec",
                    role: "spec-writer",
                    pod: None,
                    workspace: root,
                    task: format!(
                        "Draft or revise the Dialec spec artifact for this user goal.\n\nGoal:\n{goal}\n\nWrite the full spec to `{}`. Address all open objections in `.dialec/session/objections.jsonl`. End with a convergence signal.",
                        draft.display()
                    ),
                    artifacts: existing(vec![
                        phase_dir.join("goal.md"),
                        phase_dir.join(format!("review-{}.md", round.saturating_sub(1))),
                    ]),
                    sandbox: "workspace-write",
                    approval: "never",
                },
            )?;
            promote_final_message(root, &tx, &draft)?;
        }

        let review = phase_dir.join(format!("review-{round}.md"));
        let tx = run_role(
            root,
            config,
            RoleRun {
                phase: "spec",
                role: "spec-reviewer",
                pod: None,
                workspace: root,
                task: format!(
                    "Adversarially review `{}` against the user goal. Write the review report to `{}`. Reject for correctness, completeness, clarity, architecture, or intent mismatch blockers. End with a convergence signal.",
                    draft.display(),
                    review.display()
                ),
                artifacts: existing(vec![phase_dir.join("goal.md"), draft.clone()]),
                sandbox: "read-only",
                approval: "never",
            },
        )?;
        promote_final_message(root, &tx, &review)?;
        if signal_converged(&tx.signal)
            && Ledger::read(root)?.open_blocking("spec", None).is_empty()
        {
            require_user_approval(root, state, "spec", None)?;
            let final_spec = phase_dir.join("final.md");
            fs::copy(&draft, &final_spec)
                .with_context(|| format!("failed to freeze spec at {}", final_spec.display()))?;
            write_gate_record(
                root,
                state,
                "spec",
                None,
                json!({
                    "checks": {
                        "latestReviewApproved": true,
                        "openBlockingObjections": 0,
                        "userApprovalRecorded": user_approval_exists(root, "spec", None),
                        "frozenSpec": final_spec,
                    },
                    "round": round
                }),
            )?;
            update_memory(root, "spec", &[final_spec.clone(), review.clone()])?;
            mark_phase(state, "spec", "converged");
            state.current_phase = "implement".to_string();
            write_state(root, state)?;
            log_timeline(
                root,
                json!({"event": "phase-converged", "phase": "spec", "round": round, "at": Utc::now()}),
            )?;
            return Ok(());
        }
    }

    if config.convergence.use_arbiter
        && run_arbiter(
            root,
            config,
            "spec",
            None,
            root,
            existing(vec![phase_dir.join("goal.md")]),
        )?
    {
        let final_spec = phase_dir.join("final.md");
        let latest_draft = phase_dir.join(format!("draft-{max_rounds}.md"));
        if latest_draft.exists() {
            fs::copy(&latest_draft, &final_spec)?;
        }
        write_gate_record(
            root,
            state,
            "spec",
            None,
            json!({
                "checks": {
                    "convergedByArbiter": true,
                    "frozenSpec": final_spec
                },
                "round": max_rounds
            }),
        )?;
        mark_phase(state, "spec", "converged-by-arbiter");
        state.current_phase = "implement".to_string();
        write_state(root, state)?;
        return Ok(());
    }

    handle_deadlock(root, state, "spec", None, max_rounds)
}

fn drive_implementation(
    root: &Path,
    config: &Config,
    state: &mut DialecState,
    max_rounds: u32,
    max_parallel: usize,
) -> Result<()> {
    if !git::is_git_repo(root) {
        return Err(anyhow!("implementation phase requires a git repository"));
    }
    let phase_dir = dialec_dir(root).join("session").join("phase-impl");
    ensure_dir(&phase_dir)?;
    let spec = dialec_dir(root)
        .join("session")
        .join("phase-spec")
        .join("final.md");
    if !spec.exists() {
        return Err(anyhow!("missing frozen spec: {}", spec.display()));
    }
    let plan = load_or_create_pod_plan(root, &spec)?;

    let pod_results = drive_pods(root, config, plan.pods, &spec, max_rounds, max_parallel)?;
    merge_pod_results(root, config, &pod_results)?;

    drive_integration_gate(root, config, &spec)?;

    write_gate_record(
        root,
        state,
        "implement",
        None,
        json!({
            "checks": {
                "openBlockingObjections": Ledger::read(root)?.open_blocking("implement", None).len(),
                "integrationReview": dialec_dir(root).join("session").join("phase-impl").join("final-integration-review.md"),
                "verificationCommands": dialec_dir(root).join("session").join("phase-impl").join("verification-commands.jsonl")
            }
        }),
    )?;
    mark_phase(state, "implement", "converged");
    update_memory(
        root,
        "implement",
        &[dialec_dir(root)
            .join("session")
            .join("phase-impl")
            .join("pods.json")],
    )?;
    state.current_phase = "cleanup".to_string();
    write_state(root, state)?;
    log_timeline(
        root,
        json!({"event": "phase-converged", "phase": "implement", "at": Utc::now()}),
    )?;
    Ok(())
}

fn drive_pods(
    root: &Path,
    config: &Config,
    pods: Vec<Pod>,
    spec: &Path,
    max_rounds: u32,
    max_parallel: usize,
) -> Result<Vec<PodResult>> {
    let width = max_parallel.max(1).min(pods.len().max(1));
    if width == 1 || pods.len() <= 1 {
        return pods
            .iter()
            .map(|pod| drive_pod(root, config, pod, spec, max_rounds))
            .collect();
    }

    log_timeline(
        root,
        json!({
            "event": "parallel-pods-started",
            "phase": "implement",
            "podCount": pods.len(),
            "maxParallel": width,
            "at": Utc::now()
        }),
    )?;

    let mut results = Vec::with_capacity(pods.len());
    for chunk in pods.chunks(width) {
        let chunk_results = thread::scope(|scope| {
            let mut handles = Vec::with_capacity(chunk.len());
            for pod in chunk {
                handles.push(scope.spawn(move || drive_pod(root, config, pod, spec, max_rounds)));
            }

            let mut chunk_results = Vec::with_capacity(handles.len());
            for handle in handles {
                match handle.join() {
                    Ok(result) => chunk_results.push(result?),
                    Err(_) => return Err(anyhow!("parallel pod worker panicked")),
                }
            }
            Ok(chunk_results)
        })?;
        results.extend(chunk_results);
    }

    log_timeline(
        root,
        json!({
            "event": "parallel-pods-converged",
            "phase": "implement",
            "podCount": results.len(),
            "at": Utc::now()
        }),
    )?;
    Ok(results)
}

fn merge_pod_results(root: &Path, config: &Config, results: &[PodResult]) -> Result<()> {
    for result in results {
        log_timeline(
            root,
            json!({
                "event": "pod-merge-queued",
                "phase": "implement",
                "pod": result.name,
                "branch": result.branch,
                "at": Utc::now()
            }),
        )?;
        merge_or_escalate(
            root,
            &result.branch,
            &format!("dialec merge pod {}", result.name),
        )?;
        log_timeline(
            root,
            json!({"event": "pod-integrated", "phase": "implement", "pod": result.name, "branch": result.branch, "at": Utc::now()}),
        )?;
        if config.workspaces.keep_failed_workspaces {
            let _ = git::remove_worktree(root, &result.worktree_name, false);
        } else {
            let _ = git::remove_worktree(root, &result.worktree_name, true);
        }
    }
    Ok(())
}

fn drive_pod(
    root: &Path,
    config: &Config,
    pod: &Pod,
    spec: &Path,
    max_rounds: u32,
) -> Result<PodResult> {
    let pod_dir = dialec_dir(root)
        .join("session")
        .join("phase-impl")
        .join(format!("pod-{}", sanitize(&pod.name)));
    ensure_dir(&pod_dir.join("impl"))?;
    let spec_slice = pod_dir.join("spec-slice.md");
    if !spec_slice.exists() {
        fs::write(&spec_slice, &pod.spec_slice)?;
    }

    let impl_name = format!("pod-{}-impl", sanitize(&pod.name));
    let impl_worktree = git::create_worktree(root, &impl_name, None)?;
    let impl_branch = format!("dialec/{impl_name}");

    for round in 1..=max_rounds {
        let status = pod_dir
            .join("impl")
            .join(format!("round-{round}-status.md"));
        let tx = run_role(
            root,
            config,
            RoleRun {
                phase: "implement",
                role: "implementer",
                pod: Some(&pod.name),
                workspace: &impl_worktree,
                task: format!(
                    "Implement pod `{}` from `{}` against frozen spec `{}`. Write implementation status to `{}`. Fix any open objections for this pod. End with a convergence signal.",
                    pod.name,
                    spec_slice.display(),
                    spec.display(),
                    status.display()
                ),
                artifacts: existing(vec![spec.to_path_buf(), spec_slice.clone()]),
                sandbox: "workspace-write",
                approval: "never",
            },
        )?;
        promote_final_message(root, &tx, &status)?;
        let _ = git::commit_all(
            &impl_worktree,
            &format!("dialec {} implementation round {round}", pod.name),
        )?;

        let verify_name = format!("pod-{}-verify", sanitize(&pod.name));
        let _ = git::remove_worktree(root, &verify_name, true);
        let verify_worktree = git::create_worktree(root, &verify_name, Some(&impl_branch))?;
        let verify_report = pod_dir.join("impl").join(format!("verify-{round}.md"));
        let tx = run_role(
            root,
            config,
            RoleRun {
                phase: "implement",
                role: "verifier",
                pod: Some(&pod.name),
                workspace: &verify_worktree,
                task: format!(
                    "Verify pod `{}` against frozen spec `{}`. Run relevant tests/builds if available. Write report to `{}`. Do not make source changes that should be accepted. End with a convergence signal.",
                    pod.name,
                    spec.display(),
                    verify_report.display()
                ),
                artifacts: existing(vec![spec.to_path_buf(), spec_slice.clone(), status.clone()]),
                sandbox: "workspace-write",
                approval: "never",
            },
        )?;
        promote_final_message(root, &tx, &verify_report)?;
        let _ = git::remove_worktree(root, &verify_name, true);

        if !signal_converged(&tx.signal)
            || !Ledger::read(root)?
                .open_blocking("implement", Some(&pod.name))
                .is_empty()
        {
            continue;
        }

        if drive_post_impl_reviews(root, config, pod, spec, &impl_worktree, &pod_dir, round)? {
            log_timeline(
                root,
                json!({"event": "pod-converged", "phase": "implement", "pod": pod.name, "branch": impl_branch, "round": round, "at": Utc::now()}),
            )?;
            return Ok(PodResult {
                name: pod.name.clone(),
                branch: impl_branch,
                worktree_name: impl_name,
            });
        }
    }

    if config.convergence.use_arbiter
        && run_arbiter(
            root,
            config,
            "implement",
            Some(&pod.name),
            &impl_worktree,
            existing(vec![spec.to_path_buf(), spec_slice]),
        )?
        && Ledger::read(root)?
            .open_blocking("implement", Some(&pod.name))
            .is_empty()
    {
        return Ok(PodResult {
            name: pod.name.clone(),
            branch: impl_branch,
            worktree_name: impl_name,
        });
    }

    handle_pod_deadlock(root, "implement", &pod.name, max_rounds)?;
    unreachable!("handle_pod_deadlock always returns an error")
}

fn drive_post_impl_reviews(
    root: &Path,
    config: &Config,
    pod: &Pod,
    spec: &Path,
    impl_worktree: &Path,
    pod_dir: &Path,
    round: u32,
) -> Result<bool> {
    let meta = pod_dir.join("meta-verify.md");
    let deslop = pod_dir.join("deslop.md");
    let tx_meta = run_role(
        root,
        config,
        RoleRun {
            phase: "implement",
            role: "meta-verifier",
            pod: Some(&pod.name),
            workspace: impl_worktree,
            task: format!(
                "Review whether verifier coverage for pod `{}` was thorough enough. Write report to `{}`. End with a convergence signal.",
                pod.name,
                meta.display()
            ),
            artifacts: existing(vec![
                spec.to_path_buf(),
                pod_dir.join("impl").join(format!("verify-{round}.md")),
            ]),
            sandbox: "read-only",
            approval: "never",
        },
    )?;
    promote_final_message(root, &tx_meta, &meta)?;

    let tx_deslop = run_role(
        root,
        config,
        RoleRun {
            phase: "implement",
            role: "deslopper",
            pod: Some(&pod.name),
            workspace: impl_worktree,
            task: format!(
                "Review implementation quality for pod `{}`: over-abstraction, dead code, inconsistency, AI slop, and fit to local patterns. Write report to `{}`. End with a convergence signal.",
                pod.name,
                deslop.display()
            ),
            artifacts: existing(vec![spec.to_path_buf(), meta.clone()]),
            sandbox: "read-only",
            approval: "never",
        },
    )?;
    promote_final_message(root, &tx_deslop, &deslop)?;

    if signal_converged(&tx_meta.signal)
        && signal_converged(&tx_deslop.signal)
        && Ledger::read(root)?
            .open_blocking("implement", Some(&pod.name))
            .is_empty()
    {
        return Ok(true);
    }

    let response = pod_dir.join("response.md");
    let tx_response = run_role(
        root,
        config,
        RoleRun {
            phase: "implement",
            role: "implementer",
            pod: Some(&pod.name),
            workspace: impl_worktree,
            task: format!(
                "Respond to meta-verifier and deslopper findings for pod `{}`. Fix valid blockers, defend correct choices with evidence, and write response to `{}`. End with a convergence signal.",
                pod.name,
                response.display()
            ),
            artifacts: existing(vec![spec.to_path_buf(), meta, deslop]),
            sandbox: "workspace-write",
            approval: "never",
        },
    )?;
    promote_final_message(root, &tx_response, &response)?;
    let _ = git::commit_all(
        impl_worktree,
        &format!("dialec {} post-review fixes", pod.name),
    )?;
    Ok(signal_converged(&tx_response.signal)
        && Ledger::read(root)?
            .open_blocking("implement", Some(&pod.name))
            .is_empty())
}

fn drive_cleanup(
    root: &Path,
    config: &Config,
    state: &mut DialecState,
    max_rounds: u32,
) -> Result<()> {
    if !git::is_git_repo(root) {
        return Err(anyhow!("cleanup phase requires a git repository"));
    }
    let phase_dir = dialec_dir(root).join("session").join("phase-cleanup");
    ensure_dir(&phase_dir)?;
    let spec = dialec_dir(root)
        .join("session")
        .join("phase-spec")
        .join("final.md");

    let cleanup_name = "cleanup-refactor";
    let cleanup_worktree = git::create_worktree(root, cleanup_name, None)?;
    let cleanup_branch = format!("dialec/{cleanup_name}");

    let analysis = phase_dir.join("analysis.md");
    if !analysis.exists() {
        let tx = run_role(
            root,
            config,
            RoleRun {
                phase: "cleanup",
                role: "refactorer",
                pod: None,
                workspace: &cleanup_worktree,
                task: format!(
                    "Analyze implementation for LLM maintainability. Flag ambiguous intent, invalid tests, dead abstractions, and decomposition opportunities. Write analysis to `{}`. End with a convergence signal.",
                    analysis.display()
                ),
                artifacts: existing(vec![spec.clone()]),
                sandbox: "workspace-write",
                approval: "never",
            },
        )?;
        promote_final_message(root, &tx, &analysis)?;
    }

    if state.mode == "interactive" || state.mode == "sidecar" {
        let user_input = phase_dir.join("user-input.md");
        if !user_input.exists() {
            fs::write(
                &user_input,
                "# User Cleanup Input\n\nReview `analysis.md` and replace this file with confirmations/rejections for ambiguous intent, invalid tests, and backward-compat breaks before rerunning `dialec drive`.\n",
            )?;
            log_timeline(
                root,
                json!({"event": "user-input-required", "phase": "cleanup", "path": user_input, "at": Utc::now()}),
            )?;
            return Err(anyhow!(
                "cleanup requires user input at {}",
                user_input.display()
            ));
        }
    }

    for round in 1..=max_rounds {
        let changes = phase_dir.join(format!("changes-{round}.md"));
        let tx = run_role(
            root,
            config,
            RoleRun {
                phase: "cleanup",
                role: "refactorer",
                pod: None,
                workspace: &cleanup_worktree,
                task: format!(
                    "Execute cleanup/refactor round {round} using `{}` and optional user input. Write changes report to `{}`. End with a convergence signal.",
                    analysis.display(),
                    changes.display()
                ),
                artifacts: existing(vec![
                    spec.clone(),
                    analysis.clone(),
                    phase_dir.join("user-input.md"),
                ]),
                sandbox: "workspace-write",
                approval: "never",
            },
        )?;
        promote_final_message(root, &tx, &changes)?;
        let _ = git::commit_all(&cleanup_worktree, &format!("dialec cleanup round {round}"))?;

        let review = phase_dir.join(format!("adversarial-review-{round}.md"));
        let tx = run_role(
            root,
            config,
            RoleRun {
                phase: "cleanup",
                role: "adversary",
                pod: None,
                workspace: &cleanup_worktree,
                task: format!(
                    "Adversarially review cleanup round {round}. Check behavior preservation, intent loss, and whether decomposition is actually better. Write report to `{}`. End with a convergence signal.",
                    review.display()
                ),
                artifacts: existing(vec![spec.clone(), analysis.clone(), changes.clone()]),
                sandbox: "workspace-write",
                approval: "never",
            },
        )?;
        promote_final_message(root, &tx, &review)?;
        if signal_converged(&tx.signal)
            && Ledger::read(root)?
                .open_blocking("cleanup", None)
                .is_empty()
        {
            run_verification_commands(root, &phase_dir.join("verification-commands.jsonl"), &spec)?;
            merge_or_escalate(root, &cleanup_branch, "dialec merge cleanup")?;
            if config.workspaces.keep_failed_workspaces {
                let _ = git::remove_worktree(root, cleanup_name, false);
            } else {
                let _ = git::remove_worktree(root, cleanup_name, true);
            }
            update_memory(root, "cleanup", &[analysis.clone(), review.clone()])?;
            write_gate_record(
                root,
                state,
                "cleanup",
                None,
                json!({
                    "checks": {
                        "openBlockingObjections": 0,
                        "userInputRecorded": phase_dir.join("user-input.md").exists(),
                        "verificationCommands": phase_dir.join("verification-commands.jsonl")
                    },
                    "round": round
                }),
            )?;
            mark_phase(state, "cleanup", "converged");
            state.current_phase = "done".to_string();
            write_state(root, state)?;
            log_timeline(
                root,
                json!({"event": "phase-converged", "phase": "cleanup", "round": round, "at": Utc::now()}),
            )?;
            return Ok(());
        }
    }

    if config.convergence.use_arbiter
        && run_arbiter(
            root,
            config,
            "cleanup",
            None,
            &cleanup_worktree,
            existing(vec![spec, analysis]),
        )?
        && Ledger::read(root)?
            .open_blocking("cleanup", None)
            .is_empty()
    {
        merge_or_escalate(root, &cleanup_branch, "dialec merge cleanup")?;
        write_gate_record(
            root,
            state,
            "cleanup",
            None,
            json!({
                "checks": {
                    "convergedByArbiter": true
                },
                "round": max_rounds
            }),
        )?;
        mark_phase(state, "cleanup", "converged-by-arbiter");
        state.current_phase = "done".to_string();
        write_state(root, state)?;
        return Ok(());
    }

    handle_deadlock(root, state, "cleanup", None, max_rounds)
}

struct RoleRun<'a> {
    phase: &'a str,
    role: &'a str,
    pod: Option<&'a str>,
    workspace: &'a Path,
    task: String,
    artifacts: Vec<PathBuf>,
    sandbox: &'a str,
    approval: &'a str,
}

fn run_role(root: &Path, config: &Config, run: RoleRun<'_>) -> Result<RunTransaction> {
    let harness = config
        .roles
        .get(run.role)
        .ok_or_else(|| anyhow!("role {} has no harness mapping", run.role))?;
    let tx = run_transaction(RunRequest {
        phase: run.phase.to_string(),
        role: run.role.to_string(),
        harness: harness.clone(),
        task: run.task,
        workspace: run.workspace.to_path_buf(),
        project_root: root.to_path_buf(),
        pod: run.pod.map(str::to_string),
        sandbox: run.sandbox.to_string(),
        approval: run.approval.to_string(),
        timeout_ms: 1_800_000,
        artifacts: run.artifacts,
        max_budget_usd: Some(config.budget.per_turn_max_usd),
        max_turns: config.budget.max_turns,
    })?;
    log_timeline(
        root,
        json!({
            "event": "turn-completed",
            "transactionId": tx.id,
            "phase": tx.phase,
            "pod": tx.pod,
            "role": tx.role,
            "harness": tx.harness,
            "verdict": tx.signal.verdict,
            "exitCode": tx.exit_code,
            "at": tx.completed_at
        }),
    )?;
    log_cost(
        root,
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
    Ok(tx)
}

fn promote_final_message(root: &Path, tx: &RunTransaction, dest: &Path) -> Result<()> {
    if dest.exists() {
        return Ok(());
    }
    if let Some(parent) = dest.parent() {
        ensure_dir(parent)?;
    }
    let source = root.join(&tx.final_message.path);
    if source.exists() {
        fs::copy(&source, dest).with_context(|| {
            format!(
                "failed to promote {} to {}",
                source.display(),
                dest.display()
            )
        })?;
    }
    Ok(())
}

fn load_or_create_pod_plan(root: &Path, spec: &Path) -> Result<PodPlan> {
    let plan_path = dialec_dir(root)
        .join("session")
        .join("phase-impl")
        .join("pods.json");
    if plan_path.exists() {
        let data = fs::read(&plan_path)?;
        return Ok(serde_json::from_slice(&data)?);
    }
    let spec_text = fs::read_to_string(spec).unwrap_or_default();
    let plan = PodPlan {
        pods: vec![Pod {
            name: "main".to_string(),
            spec_slice: spec_text,
        }],
    };
    write_json_pretty(&plan_path, &plan)?;
    Ok(plan)
}

fn existing(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths.into_iter().filter(|path| path.exists()).collect()
}

fn mark_phase(state: &mut DialecState, phase: &str, status: &str) {
    state.phases.insert(
        phase.to_string(),
        json!({"status": status, "updatedAt": Utc::now()}),
    );
}

fn run_arbiter(
    root: &Path,
    config: &Config,
    phase: &str,
    pod: Option<&str>,
    workspace: &Path,
    artifacts: Vec<PathBuf>,
) -> Result<bool> {
    if !config.roles.contains_key("arbiter") {
        log_decision(
            root,
            json!({
                "event": "arbiter-skipped",
                "phase": phase,
                "pod": pod,
                "reason": "no arbiter role mapping",
                "at": Utc::now()
            }),
        )?;
        return Ok(false);
    }
    let blockers = Ledger::read(root)?.open_blocking(phase, pod);
    if blockers.is_empty() {
        return Ok(true);
    }
    let tx = run_role(
        root,
        config,
        RoleRun {
            phase,
            role: "arbiter",
            pod,
            workspace,
            task: format!(
                "Arbitrate unresolved blocking objections for phase `{phase}` pod `{}`. Read `.dialec/session/objections.jsonl`. Decide whether each remaining objection is genuinely blocking. For objections that are not genuinely blocking, include their ids in `resolvedObjectionIds`. Do not mutate source code. End with a convergence signal.",
                pod.unwrap_or("global")
            ),
            artifacts,
            sandbox: "read-only",
            approval: "never",
        },
    )?;
    Ok(signal_converged(&tx.signal) && Ledger::read(root)?.open_blocking(phase, pod).is_empty())
}

fn require_user_approval(
    root: &Path,
    state: &DialecState,
    phase: &str,
    pod: Option<&str>,
) -> Result<()> {
    if state.mode == "autonomous" {
        return Ok(());
    }
    if user_approval_exists(root, phase, pod) {
        return Ok(());
    }
    let approval_path = approval_path(root, phase, pod);
    if let Some(parent) = approval_path.parent() {
        ensure_dir(parent)?;
    }
    fs::write(
        &approval_path,
        format!(
            "# User Approval Required\n\nReview the `{phase}` artifacts and replace this file with the user's approval, rejection, or requested changes before advancing.\n"
        ),
    )?;
    log_timeline(
        root,
        json!({
            "event": "user-approval-required",
            "phase": phase,
            "pod": pod,
            "path": approval_path,
            "at": Utc::now()
        }),
    )?;
    Err(anyhow!(
        "{phase} requires user approval at {}",
        approval_path.display()
    ))
}

fn user_approval_exists(root: &Path, phase: &str, pod: Option<&str>) -> bool {
    let path = approval_path(root, phase, pod);
    path.exists()
        && fs::read_to_string(path)
            .map(|content| {
                let lower = content.to_ascii_lowercase();
                lower.contains("approve")
                    || lower.contains("approved")
                    || lower.contains("user-accepted")
            })
            .unwrap_or(false)
}

fn approval_path(root: &Path, phase: &str, pod: Option<&str>) -> PathBuf {
    match (phase, pod) {
        ("implement", Some(pod)) => dialec_dir(root)
            .join("session")
            .join("phase-impl")
            .join(format!("pod-{}", sanitize(pod)))
            .join("user-approval.md"),
        _ => dialec_dir(root)
            .join("session")
            .join(format!("phase-{phase}"))
            .join("user-approval.md"),
    }
}

fn write_gate_record(
    root: &Path,
    state: &DialecState,
    phase: &str,
    pod: Option<&str>,
    details: serde_json::Value,
) -> Result<()> {
    let gate_name = match pod {
        Some(pod) => format!("{}-{}-gate", phase, sanitize(pod)),
        None => format!("{phase}-gate"),
    };
    let gate_dir = dialec_dir(root)
        .join("session")
        .join("gates")
        .join(gate_name);
    ensure_dir(&gate_dir)?;
    let record = json!({
        "event": "phase-gate",
        "phase": phase,
        "pod": pod,
        "mode": state.mode,
        "sessionId": state.session_id,
        "details": details,
        "at": Utc::now()
    });
    write_json_pretty(&gate_dir.join("gate.json"), &record)?;
    log_timeline(root, record)?;
    Ok(())
}

fn drive_integration_gate(root: &Path, config: &Config, spec: &Path) -> Result<()> {
    let phase_dir = dialec_dir(root).join("session").join("phase-impl");
    ensure_dir(&phase_dir)?;
    let verification_report = phase_dir.join("verification-commands.jsonl");
    run_verification_commands(root, &verification_report, spec)?;
    let integration_review = phase_dir.join("final-integration-review.md");
    let tx = run_role(
        root,
        config,
        RoleRun {
            phase: "implement",
            role: "verifier",
            pod: None,
            workspace: root,
            task: format!(
                "Run the final integrated implementation gate against frozen spec `{}`. Review the integrated workspace, consider pod merge interactions, inspect verification command results at `{}`, and write the report to `{}`. End with a convergence signal.",
                spec.display(),
                verification_report.display(),
                integration_review.display()
            ),
            artifacts: existing(vec![spec.to_path_buf(), verification_report]),
            sandbox: "workspace-write",
            approval: "never",
        },
    )?;
    promote_final_message(root, &tx, &integration_review)?;
    if signal_converged(&tx.signal)
        && Ledger::read(root)?
            .open_blocking("implement", None)
            .is_empty()
    {
        Ok(())
    } else {
        Err(anyhow!(
            "final integrated implementation gate rejected; inspect {}",
            integration_review.display()
        ))
    }
}

fn run_verification_commands(root: &Path, report_path: &Path, spec: &Path) -> Result<()> {
    if report_path.exists() {
        return Ok(());
    }
    let commands = extract_verification_commands(spec)?;
    if commands.is_empty() {
        fs::write(
            report_path,
            serde_json::to_string(&json!({
                "event": "verification-commands",
                "status": "unknown",
                "reason": "no explicit verification commands found in frozen spec",
                "at": Utc::now()
            }))? + "\n",
        )?;
        return Ok(());
    }
    let mut lines = String::new();
    for command in commands {
        let started = Utc::now();
        let output = Command::new("sh")
            .arg("-lc")
            .arg(&command)
            .current_dir(root)
            .output()
            .with_context(|| format!("failed to run verification command `{command}`"))?;
        let status = output.status.code().unwrap_or(-1);
        lines.push_str(&serde_json::to_string(&json!({
            "event": "verification-command",
            "command": command,
            "startedAt": started,
            "completedAt": Utc::now(),
            "exitCode": status,
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        }))?);
        lines.push('\n');
        if !output.status.success() {
            fs::write(report_path, lines)?;
            return Err(anyhow!("verification command failed: `{command}`"));
        }
    }
    fs::write(report_path, lines)?;
    Ok(())
}

fn extract_verification_commands(spec: &Path) -> Result<Vec<String>> {
    let text = fs::read_to_string(spec).unwrap_or_default();
    let mut commands = vec![];
    let mut in_fence = false;
    let mut fence_is_shell = false;
    let mut in_verification_section = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            let lower = trimmed.to_ascii_lowercase();
            in_verification_section = lower.contains("verification")
                || lower.contains("test command")
                || lower.contains("checks");
        }
        if trimmed.starts_with("```") {
            if in_fence {
                in_fence = false;
                fence_is_shell = false;
            } else {
                in_fence = true;
                let lower = trimmed.to_ascii_lowercase();
                fence_is_shell = in_verification_section
                    && (lower.contains("sh")
                        || lower.contains("bash")
                        || lower == "```"
                        || lower.contains("shell"));
            }
            continue;
        }
        if in_fence && fence_is_shell && !trimmed.is_empty() && !trimmed.starts_with('#') {
            commands.push(trimmed.trim_start_matches("$ ").to_string());
            continue;
        }
        if in_verification_section {
            for prefix in ["- `$ ", "- `", "* `$ ", "* `"] {
                if let Some(rest) = trimmed.strip_prefix(prefix)
                    && let Some(command) = rest.strip_suffix('`')
                {
                    commands.push(command.trim_start_matches("$ ").to_string());
                }
            }
        }
    }
    commands.sort();
    commands.dedup();
    Ok(commands)
}

fn merge_or_escalate(root: &Path, branch: &str, message: &str) -> Result<()> {
    match git::merge_branch(root, branch, message) {
        Ok(()) => Ok(()),
        Err(error) => {
            let escalation = dialec_dir(root).join("session").join("escalation.md");
            fs::write(
                &escalation,
                format!(
                    "# Dialec Merge Escalation\n\nBranch: `{branch}`\nMessage: `{message}`\n\nError:\n\n```text\n{error}\n```\n"
                ),
            )?;
            log_decision(
                root,
                json!({
                    "event": "merge-conflict",
                    "branch": branch,
                    "message": message,
                    "error": error.to_string(),
                    "escalation": escalation,
                    "at": Utc::now()
                }),
            )?;
            Err(error)
        }
    }
}

fn update_memory(root: &Path, phase: &str, artifacts: &[PathBuf]) -> Result<()> {
    let memory_dir = dialec_dir(root).join("memory");
    ensure_dir(&memory_dir)?;
    let decisions = memory_dir.join("decisions.md");
    let mut entry = format!(
        "\n## Phase `{phase}` converged at {}\n\nArtifacts:\n",
        Utc::now().to_rfc3339()
    );
    for artifact in artifacts.iter().filter(|path| path.exists()) {
        entry.push_str(&format!("- `{}`\n", artifact.display()));
    }
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&decisions)
        .with_context(|| format!("failed to update {}", decisions.display()))?;
    file.write_all(entry.as_bytes())?;
    Ok(())
}

fn handle_deadlock(
    root: &Path,
    state: &mut DialecState,
    phase: &str,
    pod: Option<&str>,
    round: u32,
) -> Result<()> {
    let blockers = Ledger::read(root)?.open_blocking(phase, pod);
    log_decision(
        root,
        json!({
            "event": "deadlock",
            "phase": phase,
            "pod": pod,
            "round": round,
            "mode": state.mode,
            "openBlocking": blockers,
            "at": Utc::now()
        }),
    )?;
    if state.mode == "autonomous" {
        return Err(anyhow!(
            "autonomous mode failed closed on {phase} deadlock with {} blocker(s)",
            blockers.len()
        ));
    }
    Err(anyhow!(
        "interactive deadlock in {phase}; inspect .dialec/session/objections.jsonl"
    ))
}

fn handle_pod_deadlock(root: &Path, phase: &str, pod: &str, round: u32) -> Result<()> {
    let blockers = Ledger::read(root)?.open_blocking(phase, Some(pod));
    log_decision(
        root,
        json!({
            "event": "pod-deadlock",
            "phase": phase,
            "pod": pod,
            "round": round,
            "openBlocking": blockers,
            "at": Utc::now()
        }),
    )?;
    Err(anyhow!(
        "pod {pod} deadlocked in {phase}; inspect .dialec/session/objections.jsonl"
    ))
}
