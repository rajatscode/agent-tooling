# Dialec

Rust CLI for the Dialec harness spec. It keeps implementation files under
`dialec/`; runtime state for a target project goes in that project's
`.dialec/` directory.

## Local Setup

```bash
cd dialec
cargo build --release
cargo install --path .
```

After `cargo install --path .`, the executable is available as `dialec` from
Cargo's bin directory, usually `~/.cargo/bin`.

If you only want to run it from this checkout:

```bash
cd dialec
cargo run -- --help
```

## First Use In A Project

Run these from the project you want Dialec to operate on:

```bash
dialec init
dialec check
dialec start --mode sidecar --goal "implement auth"
dialec status
```

`sidecar` is the default. It creates `.dialec/`, writes the Claude Code
sidecar skill at `.dialec/skills/sidecar.md`, and expects the live Claude
session to coordinate with `dialec run`, `dialec spec`, `dialec implement`,
and `dialec cleanup`.

For headless execution:

```bash
dialec start --mode autonomous --goal "implement auth" --budget 4h
dialec tail --coordinator --follow
```

Autonomous mode spawns a headless Claude coordinator with
`.dialec/roles/coordinator.md`; it records coordinator stdout/stderr under
`.dialec/log/` and preserves all harness turns as normal transactions.

For deterministic local autopilot from your shell:

```bash
dialec start --mode sidecar --goal "implement auth" --drive
```

In sidecar/interactive mode this still stops at user-approval gates. Use
autonomous mode for headless gate decisions, or resume an existing session
with:

```text
dialec drive
spec -> implement -> cleanup
```

Phase-by-phase:

```bash
dialec spec
dialec implement
dialec cleanup
```

`dialec check` probes installed harnesses and writes capability reports to:

```text
.dialec/capabilities/
```

## Running A Turn

Manual single-turn execution is still available for debugging. This invokes
the selected native harness and may spend API/model budget:

```bash
dialec run \
  --harness codex \
  --role implementer \
  --phase implement \
  --task "Implement the frozen spec in .dialec/session/phase-spec/final.md" \
  --workspace . \
  --artifact .dialec/session/phase-spec/final.md
```

Every run is recorded as an immutable transaction:

```text
.dialec/session/turns/0001-codex-implementer/
├── input.json
├── command.json
├── before.json
├── reminder.md
├── stdout.log
├── stderr.log
├── events.jsonl
├── final-message.md
├── structured.json
├── signal.json
├── after.json
├── patch.diff
└── transaction.json
```

## Worktrees

```bash
dialec worktree create pod-auth
dialec worktree list
dialec worktree remove pod-auth --delete-branch
```

## Phase Runner

```bash
dialec drive                 # continue from current phase until done
dialec spec                  # run only spec phase
dialec implement --max-parallel 4
dialec cleanup               # run only cleanup phase
dialec advance --reason "ship this decision"
dialec retry --hint "focus on the auth edge case"
```

The runner calls the transaction engine internally, checks convergence
signals, updates `.dialec/session/objections.jsonl`, and advances phase
state in `.dialec/dialec.json`.

Implementation pods run concurrently when `--max-parallel` is greater than
one. Each pod converges in its own Dialec-managed worktree; converged pod
branches then enter an ordered merge queue so the main workspace is only
written by one merge at a time. Timeline events include
`parallel-pods-started`, `parallel-pods-converged`, `pod-merge-queued`, and
`pod-integrated`.

`drive` is intentionally explicit. It goes beyond the original "Dialec is
mostly plumbing" model by acting as a deterministic local conductor, but it
uses the same audited transaction, ledger, gate, and worktree machinery as
sidecar and autonomous mode.

## Role Reminder Crons

Dialec injects role/rules reminders into every turn by default. Each reminder
is written as `reminder.md` inside the transaction directory and logged as a
`role-reminder` timeline event.

```bash
dialec cron list
dialec cron tick --role project-manager --phase spec
```

Configure cadence and role rules in `.dialec/config.json` under
`reminders`. Defaults include role boundaries such as PM/coordinator/reviewer
roles not writing source code, implementers owning pod code changes, and
verifier/adversary worktrees being disposable.

## Transparency

```bash
dialec status --json
dialec log --phase implement
dialec tail --turn 0003-codex-implementer --stream events
dialec tail --coordinator --stream stderr
```

Phase gates are recorded under `.dialec/session/gates/`. Merge conflicts and
autonomous deadlocks write `.dialec/session/escalation.md`.

Harness stdout is normalized into `events.jsonl` with categories such as
`message`, `tool-call`, `tool-result`, `command`, `file-change`, `session`,
and `cost`. Claude, Codex, Gemini, Cursor, and Claudish each have adapter-aware
normalization with a generic fallback for unknown event shapes.

When a harness reports a session id and capability probing confirms resume
support, Dialec stores it in `.dialec/session/resume.json` and reuses it on the
next compatible harness/role/phase/pod turn. Artifacts, memory, reminders, and
the objection ledger are still replayed every time; resume is only a context
cache.

## Custom Workflows

The built-in workflow is `spec -> implement -> cleanup`. Additional phase DAGs
can be added as JSON files in `.dialec/workflows/<name>.json` or under
`workflows` in `.dialec/config.json`.

```bash
dialec workflow list
dialec workflow show default
dialec workflow run default
```

## Notes

- `dialec run` uses real harness CLIs: `claude`, `codex`, `gemini`,
  `cursor-agent`, and optional `claudish`.
- Codex `--json` is captured as JSONL events; Claude/Gemini/Cursor JSON output
  is normalized into `events.jsonl` when parseable.
- Cursor is resolved as `cursor-agent`, not a local `agent` helper.
- Runtime `.dialec/` state belongs to the target project, not this source
  directory.
