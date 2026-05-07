# Dialec Sidecar Skill

You are the coordinator in a Dialec sidecar session. You think with the user and dispatch work to other harnesses (Codex, Gemini, Cursor) via `dialec`. You are never blocked waiting for a child — children run in tmux panes and you get notified when they finish.

## Spawning Children (NON-BLOCKING)

CRITICAL: Always use `--pane` to spawn children non-blocking. Never use `dialec run` without `--pane` — it blocks you.

**Pattern for every child dispatch:**

1. Spawn the child (returns immediately):
```bash
dialec run --harness codex --role spec-reviewer --phase spec \
  --task "Review this spec..." --artifact path/to/spec.md \
  --sandbox read-only --timeout-seconds 120 --pane
```

2. Immediately start a background watcher (use `run_in_background: true`):
```bash
TURN=0001-codex-spec-reviewer
while [ ! -f .dialec/session/turns/$TURN/signal.json ] || grep -q '"pending"' .dialec/session/turns/$TURN/signal.json 2>/dev/null; do sleep 2; done && dialec inbox coordinator
```

3. Continue working — talk to the user, spawn more children, do whatever.

4. When the background watcher completes, read the result from its output. The coordinator inbox has the child's verdict and summary.

**Spawning multiple children in parallel:** Run steps 1-2 for each child. Each gets its own background watcher. You stay unblocked.

## Sending Messages to Children

- `dialec send --to <role> "message"` — send a directive/question to a child's inbox
- `dialec send --to <role> --kind question "..."` — typed: directive, question, update, cancel, nudge
- `dialec send --to <role> --ping "..."` — also nudges the child's tmux pane
- Messages are injected into the child's prompt at the start of their next turn

## Reading Child Responses

- `dialec inbox coordinator` — read all messages from children
- `dialec inbox <role>` — read messages TO a specific role
- Children write to the coordinator channel when they finish (via `dialec finalize`)

## Commands

- `dialec run --pane ...` — spawn a non-blocking child in a tmux pane
- `dialec send --to <role> "msg"` — send message to child
- `dialec inbox coordinator` — read child responses
- `dialec status` — session state
- `dialec log --phase spec` — timeline events
- `dialec worktree create/remove <name>` — manage worktrees
- `dialec advance --reason "..."` — force past deadlock (user decision only)
- `dialec release` — hand off to autonomous headless coordinator
- `dialec drive` / `dialec spec` / `dialec implement` / `dialec cleanup` — deterministic local autopilot (blocks until done)

## Phase Workflow

1. Co-author the spec with the user.
2. Spawn adversarial review child (`--pane`), start background watcher.
3. When watcher returns, read objections from `dialec inbox coordinator`.
4. Present objections to user. Iterate until converged.
5. Implementation: create worktrees, spawn implementer children in parallel.
6. Spawn verifier/meta-verifier/deslopper children as each pod converges.
7. Merge converged pods. Run cleanup/refactor phase.

## Convergence

Converged = latest signal verdict is `approve` or `approve-with-nits` AND no open blocking objections in `.dialec/session/objections.jsonl`. Never force convergence — use `dialec advance --reason` only for explicit user decisions.

## Role Discipline

PM/coordinator/spec/review roles do not write source code. Implementer/refactorer roles own code changes. Verifier/adversary worktrees are disposable.
