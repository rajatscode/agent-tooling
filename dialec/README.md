# Dialec

Multi-harness orchestrator. Coordinates Claude, Codex, Gemini, and Cursor
through structured phases (spec → implement → cleanup) with convergence,
tmux panes, and inter-agent messaging.

## Install

```bash
cd dialec
cargo install --path .
```

Requires `claude` and `codex` CLIs installed and authenticated.

```bash
dialec check   # verify harnesses are available
```

## Quick Start: Sidecar Mode (you + Claude + Codex)

You stay in your Claude Code session. Claude dispatches work to Codex (and
other harnesses) via `dialec` and gets notified when children finish.

```bash
# From your target project directory:
dialec start --mode sidecar --goal "implement auth system"
```

Then in your Claude session, use the sidecar skill
(`.dialec/skills/sidecar.md`) — Claude spawns children with `--pane`,
starts background watchers, and continues working while children run.

### The flow:

1. You and Claude write a spec together
2. Claude runs: `dialec run --pane --harness codex --role spec-reviewer ...`
   → returns instantly, Codex reviews in a tmux pane
3. Claude starts a background watcher on the turn
4. When Codex finishes, the watcher notifies Claude
5. Claude reads `dialec inbox coordinator` for Codex's verdict
6. You iterate until convergence
7. Claude dispatches implementation to Codex, verification stays with Claude
8. Repeat through cleanup/refactor

### Sending messages to children mid-session:

```bash
dialec send --to implementer "Focus on error handling, not just happy path"
dialec send --to spec-reviewer --kind question "Are you checking token expiry?"
dialec send --to implementer --ping "Check your inbox"   # also nudges the tmux pane
dialec inbox coordinator   # read responses from children
```

## Quick Start: Autonomous Overnight Mode

Claude drives end-to-end without you. Set a goal, set a time, walk away.

### "Work on this project until 8am"

```bash
cd /path/to/your/project
dialec start --mode autonomous \
  --goal "implement the auth module per docs/auth-spec.md" \
  --until "8am ET" \
  --budget '$15'
```

This spawns a headless Claude with the coordinator skill. It:
- Writes a spec (or uses one you provide)
- Sends it to Codex for adversarial review
- Iterates until convergence
- Creates worktrees, dispatches implementation pods to Codex
- Verifies, meta-verifies, deslops each pod
- Merges converged pods
- Runs cleanup/refactor with adversarial review
- **Checks if the goal was actually achieved** (not just "phases done")
- **When phases complete and time remains, asks a PM agent "what's next?"
  and loops back** — hackathon mode, keeps building until the deadline

Watch it work (optional):
```bash
dialec tail --coordinator --follow
```

Review in the morning:
```bash
dialec status
dialec log
cat .dialec/session/final-report.md
```

### Time and budget formats

```bash
--budget '$10'              # cost cap
--budget '4h'               # time cap (4 hours from now)
--until '8am ET'            # work until 8am Eastern
--until '6:30am'            # local timezone
--until '22:00 UTC'         # 24-hour UTC
--budget '$15, until 8am ET'  # combine cost + time

# Supported timezones: ET/EDT/EST, CT/CST, MT/MST, PT/PDT/PST, UTC/GMT
```

When `--until` is set, dialec enters **hackathon mode**: after completing
all phases, it asks "what should we build next?" and loops back to the
spec phase with a new goal. It keeps going until the deadline or the PM
agent says there's nothing left to do.

### Stopping conditions

The session stops when ANY of these are true:
- All phases converge AND no `work_until` time remains
- Cost budget exceeded (`--budget '$10'`)
- Time deadline reached (`--until '8am ET'`)
- Deadlock (autonomous mode fails closed on correctness/security blockers)
- PM agent says "NO_MORE_WORK" in hackathon loop
- Coordinator Claude hits its own token limit

### Goal achievement detection

After the implementation phase converges, dialec asks a verifier:
"has this goal actually been achieved?" If not, and there's time
remaining, it loops back to the spec phase to address what's missing.
This prevents the "all phases converged on the wrong thing" failure mode.

### Option B: Deterministic drive (no coordinator, dialec does it)

```bash
dialec start --mode sidecar --goal "implement auth" --no-drive
# ... write your spec to .dialec/session/phase-spec/final.md ...
dialec drive --max-parallel 4
```

`dialec drive` runs the phase machine directly in Rust — it calls
harnesses sequentially per phase, checks convergence, and advances.
No Claude coordinator involved. Stops at user-approval gates in
sidecar mode. Pass `--mode autonomous` to skip gates.

### Switching modes mid-session:

```bash
# You're in sidecar, happy with the spec, want to go to bed:
dialec release   # spawns autonomous coordinator to continue

# You wake up, coordinator is stuck:
dialec intervene   # kills coordinator, drops to sidecar mode
```

## Pane Mode

`--pane` runs harnesses in visible tmux panes. Non-blocking — returns
immediately with `verdict: pending`.

```bash
# Spawn a child (returns instantly):
dialec run --pane --harness codex --role implementer --phase implement \
  --task "Implement auth" --artifact .dialec/session/phase-spec/final.md \
  --sandbox workspace-write --timeout-seconds 300

# Watch it work in the tmux pane
# When it finishes, it auto-finalizes and notifies the coordinator channel

# Check results:
dialec inbox coordinator
```

Multiple children can run in parallel in separate panes.

## Inter-Agent Channels

Agents communicate through file-based channels at
`.dialec/channels/<role>/inbox.jsonl`.

```bash
dialec send --to spec-reviewer "Be harsh on missing error variants"
dialec send --to implementer --kind cancel "Stop, spec changed"
dialec inbox coordinator   # messages FROM children
dialec inbox implementer   # messages TO implementer
```

Message kinds: `directive`, `question`, `update`, `cancel`, `nudge`.

Messages are injected into agent prompts at turn start. The `--ping`
flag also sends a tmux notification to the agent's pane.

## Session Structure

```
.dialec/
├── dialec.json           # session state
├── config.json           # harness config, convergence params
├── signal-schema.json    # convergence signal JSON Schema
├── capabilities/         # probed harness reports
├── roles/                # system prompts per role
├── skills/               # Claude Code skills (sidecar.md, coordinator.md)
├── channels/             # inter-agent message inboxes
├── memory/               # persistent cross-session memory
├── session/
│   ├── objections.jsonl  # convergence ledger
│   ├── turns/            # immutable transaction dirs
│   ├── phase-spec/       # spec artifacts
│   ├── phase-impl/       # impl pods + artifacts
│   └── phase-cleanup/    # cleanup artifacts
├── workspaces/           # git worktrees for pods
└── log/                  # timeline, costs, decisions
```

## All Commands

```bash
dialec init                        # create .dialec/ layout
dialec check                       # probe harnesses, validate roles
dialec start [--mode] [--goal]     # start a session
dialec status                      # current phase, cost, turns
dialec drive [--max-parallel N]    # deterministic autopilot
dialec spec / implement / cleanup  # run a single phase
dialec run --pane --harness ...    # spawn a child in a tmux pane
dialec finalize --turn <id>        # finalize a pane turn (called by pane script)
dialec send --to <role> "msg"      # send message to agent
dialec inbox <target>              # read channel messages
dialec advance --reason "..."      # force past deadlock
dialec retry --hint "..."          # retry with guidance
dialec intervene                   # kill coordinator, drop to sidecar
dialec release                     # spawn coordinator, go autonomous
dialec tail --coordinator -f       # watch coordinator logs
dialec worktree create/remove/list # manage workspaces
dialec log [--phase] [--pod]       # view timeline
dialec harnesses                   # list detected harnesses
```
