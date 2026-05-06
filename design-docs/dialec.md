# Dialec

Multi-harness orchestrator for structured agent collaboration with
convergence. Coordinates native CLI tools (Claude, Codex, Gemini, Cursor)
through phased workflows where agents negotiate via artifacts until they
converge.

## Problem

You want Claude and Codex (and others) to collaborate on real work —
not "Claude pretending to be Codex via a proxy" but actual native
harnesses with their own auth, tools, sandboxing, and strengths. The
collaboration must be structured (spec → implement → cleanup), agents
must genuinely negotiate (not just rubber-stamp), and the system must
detect when they've converged so it can advance.

Claudish solves a different problem: making Claude Code's UX work with
other backends. We want each tool's native UX and capabilities, with a
thin dialec on top managing the flow.

## Design Principles

1. **Native harnesses, not proxies.** Each agent runs in its actual CLI
   with its own auth, sandbox, and tools. Codex gets its OS-level
   sandbox. Claude gets its native tools and model behavior. Gemini gets
   grounding. The dialec owns cross-harness workspace isolation.
2. **Artifacts, not chat.** Agents communicate through files — specs,
   reviews, patches, reports. Not conversational messages. This makes
   everything inspectable, diffable, and resumable.
3. **Convergence is real.** Not "3 rounds then move on." The dialec
   evaluates structured signals from each agent to determine whether
   blocking objections remain.
4. **The dialec is dumb.** It's a state machine + process manager,
   not another LLM. It spawns agents, passes artifacts, evaluates
   convergence rules. The intelligence is in the agents.
5. **Modes are about the user, not the agents.** Interactive vs
   autonomous changes when the user gets pinged, not how agents work.
6. **Runs are transactions.** Every agent turn has a recorded input set,
   workspace state, command invocation, logs, exit status, final response,
   structured signal, and resulting patch. The dialec never infers
   what happened from prose alone.
7. **Isolation is mandatory.** Any agent that can edit code runs in an
   isolated workspace. Parallel pods are impossible without isolation;
   integration is an explicit merge/rebase/cherry-pick step owned by the
   dialec.
8. **Adapter behavior is discovered, not assumed.** Harness CLIs change.
   The dialec probes installed commands and records supported flags,
   output modes, schema support, sandbox modes, resume support, and prompt
   injection mechanisms before launching a session.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        Dialec                             │
│                                                             │
│  ┌───────────┐  ┌──────────────┐  ┌────────────────────┐  │
│  │   Phase   │  │ Convergence  │  │  Session / State   │  │
│  │  Machine  │  │   Evaluator  │  │    Persistence     │  │
│  └─────┬─────┘  └──────┬───────┘  └────────┬───────────┘  │
│        │               │                    │              │
│  ┌─────▼───────────────▼────────────────────▼───────────┐  │
│  │                  Harness Layer                         │  │
│  │                                                       │  │
│  │  ┌────────┐  ┌────────┐  ┌────────┐  ┌────────┐     │  │
│  │  │ Claude │  │ Codex  │  │ Gemini │  │ Cursor │     │  │
│  │  │Adapter │  │Adapter │  │Adapter │  │Adapter │     │  │
│  │  └────┬───┘  └────┬───┘  └────┬───┘  └────┬───┘     │  │
│  └───────┼────────────┼───────────┼───────────┼─────────┘  │
└──────────┼────────────┼───────────┼───────────┼────────────┘
           │            │           │           │
    ┌─────▼───┐  ┌────▼────┐ ┌───▼────┐ ┌───────────▼────┐
    │claude -p│  │codex    │ │gemini  │ │cursor-agent -p │
    │  (CLI)  │  │exec     │ │  -p    │ │     (CLI)      │
    └─────────┘  └─────────┘ └────────┘ └────────────────┘
```

### Components

**Phase Machine** — Drives the workflow through phases (spec →
implement → cleanup). Each phase defines: which agents participate,
what artifacts they produce, and what convergence means.

**Convergence Evaluator** — Reads structured output from agents and
determines whether blocking objections remain. Rule-based by default;
can optionally use a fast LLM (Haiku) as arbiter for ambiguous cases.

**Harness Layer** — Unified interface over each CLI. Handles:
spawning, passing artifacts via system prompt + stdin, capturing
structured output, parsing convergence signals.

**Session Persistence** — Tracks phase, round, artifacts produced,
convergence state. Allows resume after crash or user break.

## Harness Interface

Adapters are thin wrappers around real CLIs, but they are not string
templates. Each adapter has two responsibilities:

1. Probe the local harness and report concrete capabilities.
2. Execute one auditable turn and return a normalized transaction record.

The dialec may only schedule a role onto a harness if the harness
capabilities satisfy the phase contract.

```typescript
interface HarnessAdapter {
  name: string;                    // 'claude' | 'codex' | 'gemini' | 'cursor'
  probe(): Promise<HarnessCapabilities>;

  run(params: RunParams): Promise<RunTransaction>;
}

interface HarnessCapabilities {
  available: boolean;
  command?: string;
  version?: string;
  authed?: boolean;
  cwdFlag?: string;                // e.g. --cd, --worktree, --include-directories
  headless: boolean;
  outputModes: OutputMode[];       // text, json, stream-json/jsonl
  structuredOutput: StructuredOutputCapability;
  promptInjection: PromptInjectionCapability[];
  sandboxModes: SandboxMode[];
  approvalModes: string[];
  canResume: boolean;
  canReportCost: boolean;
  canStreamEvents: boolean;
  canEmitToolEvents: boolean;
  supportsExtraWritableRoots: boolean;
  limitations: string[];           // Known local/version-specific caveats
}

interface RunParams {
  task: string;                    // The prompt
  rolePrompt: ArtifactRef;         // Role instructions
  artifacts: Artifact[];          // Input artifacts (specs, code, reviews to respond to)
  workspace: WorkspaceRef;        // Isolated working directory
  tools: ToolPolicy;              // What tools to allow
  budgets: BudgetPolicy;          // Cost/time/turn caps
  outputSchema: ArtifactRef;      // JSON Schema for structured response
  sandbox: SandboxMode;           // read-only | workspace-write | danger-full-access
  approvalMode: string;           // Harness-specific approval policy
  timeoutMs: number;
  resumeFrom?: string;
}

interface RunTransaction {
  id: string;
  phase: PhaseName;
  pod?: string;
  role: RoleName;
  harness: string;
  harnessVersion?: string;
  workspace: WorkspaceRef;
  startedAt: string;
  completedAt: string;
  command: CommandInvocation;
  inputArtifacts: ArtifactRef[];
  before: WorkspaceSnapshot;
  after: WorkspaceSnapshot;
  stdout: ArtifactRef;
  stderr: ArtifactRef;
  eventLog?: ArtifactRef;         // JSONL/stream event capture when supported
  finalMessage: ArtifactRef;
  structured: ArtifactRef;        // Raw structured response
  signal: ConvergenceSignal;      // Parsed and validated structured response
  patch?: ArtifactRef;            // git diff or bundle of changes
  cost?: CostRecord;
  sessionId?: string;
  exitCode: number;
  error?: HarnessError;
}

interface Artifact {
  id: string;
  path: string;                   // Relative to session root unless absoluteRef=true
  type: ArtifactType;
  mediaType: string;
  sha256: string;
  createdBy: RoleName | 'dialec' | 'user';
  createdAt: string;
  replaces?: string;              // Previous artifact id
}

interface ArtifactRef {
  id: string;
  path: string;
  sha256: string;
  type: ArtifactType;
}

interface ConvergenceSignal {
  verdict: 'approve' | 'reject' | 'approve-with-nits';
  summary: string;
  objections: Objection[];
  resolvedObjectionIds: string[];
  newObjectionIds: string[];
}

interface Objection {
  id: string;                     // Stable across rounds until resolved
  category: 'correctness' | 'completeness' | 'clarity' | 'architecture'
          | 'intent-mismatch' | 'style' | 'security' | 'test-coverage'
          | 'operability' | 'performance';
  severity: 'blocker' | 'major' | 'minor' | 'nit';
  description: string;
  blocking: boolean;
  evidence: string;
  proposedResolution?: string;
  location?: string;             // File:line or section ref
  owner?: RoleName;
  status: 'open' | 'addressed' | 'withdrawn' | 'user-accepted';
}

type PhaseName = 'spec' | 'implement' | 'cleanup';
type RoleName = 'spec-writer' | 'spec-reviewer' | 'implementer'
  | 'verifier' | 'meta-verifier' | 'deslopper' | 'refactorer'
  | 'adversary' | string;
type OutputMode = 'text' | 'json' | 'stream-json' | 'jsonl';
type SandboxMode = 'read-only' | 'workspace-write' | 'danger-full-access';
type ArtifactType = 'spec' | 'review' | 'code' | 'patch' | 'report'
  | 'schema' | 'role-prompt' | 'stdout' | 'stderr' | 'event-log'
  | 'final-message' | 'workspace-snapshot';
```

### Capability Discovery

`dialec check` runs before every new session unless explicitly skipped.
It records a machine-readable capability report:

```
.dialec/capabilities/
├── claude.json
├── codex.json
├── gemini.json
└── cursor.json
```

Each report includes:

- Resolved executable path and version.
- Whether auth appears valid.
- Exact supported headless flags.
- Supported output modes and whether failures still emit parseable JSON.
- Supported schema/structured-output mechanisms.
- Supported sandbox/approval modes.
- How role prompts enter model context.
- How workspace root is selected.
- Whether event streams expose tool calls, command output, file edits, and
  usage/cost.
- Local limitations detected by probes.

If a required capability is missing, the role cannot be assigned to that
harness. Example: a verifier role requires at least read access plus an
ephemeral writable temp/cache area for test execution; an implementer role
requires isolated workspace write access and patch extraction.

### Transaction Model

Every harness run is persisted as an immutable transaction directory:

```
.dialec/session/turns/0007-codex-implementer/
├── input.json                  # RunParams with artifact refs
├── command.json                # argv, env allowlist, cwd, timeout
├── before.json                 # git SHA, branch, status, untracked files
├── stdout.log
├── stderr.log
├── events.jsonl                # Optional raw event stream
├── final-message.md
├── structured.json             # Raw model final JSON, if produced
├── signal.json                 # Parsed ConvergenceSignal
├── after.json                  # git SHA, branch, status, untracked files
├── patch.diff                  # Diff from before to after
└── transaction.json            # Normalized RunTransaction
```

The dialec advances only from `transaction.json`, never directly from
stdout prose. Failed or timed-out runs are still transactions. They can
produce objections, but they cannot silently mutate state outside their
recorded workspace.

### Workspace Isolation

Code-writing roles run in git worktrees:

```
.dialec/workspaces/
├── base/                       # Optional clean checkout/cache
├── pod-auth-impl/              # Implementer worktree
├── pod-auth-verify/            # Disposable verifier worktree
└── pod-auth-cleanup/           # Cleanup/refactor worktree
```

Rules:

- Each editing role owns exactly one worktree per turn.
- Reviewer/verifier worktrees are disposable; any incidental writes are
  discarded unless promoted to an explicit patch by the dialec.
- Integration happens through git operations controlled by the dialec:
  merge, rebase, cherry-pick, or patch apply.
- Parallel pods may not write the same worktree.
- Before integration, the dialec runs conflict detection and records the
  resulting merge transaction.
- A dirty user workspace is never used as an agent write target unless the
  user explicitly opts in.

### Artifact Semantics

Artifacts are content-addressed session files. For prose artifacts, the
stored file is the source of truth. For code artifacts, the source of truth
is the patch/worktree snapshot, not a `content` string in JSON.

Code-change artifacts must represent:

- Adds, edits, deletes, renames, mode changes, and binary changes.
- Generated files and untracked files.
- The base commit they apply to.
- Whether they apply cleanly to current integration head.
- Test/build commands run against them and their results.

### Adapter Implementations

**Claude:**
```bash
claude -p "$TASK" \
  --bare \
  --append-system-prompt-file .dialec/role.md \
  --output-format json \
  --json-schema .dialec/signal-schema.json \
  --allowedTools "Read,Edit,Bash,Glob,Grep" \
  --max-turns 30 \
  --max-budget-usd 2.00
```

**Codex:**
```bash
codex exec "$TASK" \
  --json \
  --sandbox workspace-write \
  --ask-for-approval never \
  --cd "$WORKTREE" \
  --output-schema .dialec/signal-schema.json \
  -o .dialec/session/turns/0007-codex-implementer/final-message.md
```
Codex `--json` is an event stream on stdout. `-o` writes the final message,
not the event log. The adapter must capture stdout separately as
`events.jsonl`.

Role prompt/context injection uses prompt text plus project instruction
files. Current Codex CLI discovers `AGENTS.md`; the adapter must verify this
with a local prompt-input probe rather than relying on a hard-coded filename.

**Gemini:**
```bash
gemini -p "$TASK" \
  --output-format json \
  --approval-mode yolo \
  --model pro
```
System prompt via `GEMINI.md` in workspace root or prepended to prompt.

**Cursor:**
```bash
cursor-agent -p "$TASK" \
  --output-format json \
  --force \
  --model opus-4.6
```
The Cursor adapter command name is `cursor-agent`, not the repo's local
`agent` worktree helper. The adapter must disambiguate both commands during
capability discovery.

### Collaboration Normalization

Each agent's system prompt includes:

```
You are collaborating with another senior coding agent acting as [ROLE].
They will provide artifacts to review. Your counterpart is thorough,
opinionated, and will push back on weak reasoning. Treat their feedback
seriously, but defend good decisions. Do not be sycophantic: if
something is wrong, say so clearly and cite evidence.
```

This is protocol normalization. We want all agents to:
- Assume competence (no hand-holding or over-explaining)
- Produce structured output (not conversational fluff)
- Be willing to disagree (not deferential)
- Preserve auditability about which harness produced which artifact

## Phase Protocol

### Phase 1: Spec

**Goal:** Produce a converged specification that both sides agree is
complete, correct, and implementable.

```
Round 1:
  Agent A (Claude, interactive): Draft spec with user
  → produces: spec/draft-1.md

Round 2:
  Agent B (Codex, headless): Adversarial review of draft
  → produces: spec/review-1.md
  → signal: approve | reject with objections

Round 3+ (if rejected):
  Agent A: Revise spec addressing objections
  → produces: spec/draft-N.md

  Agent B: Review revision
  → produces: spec/review-N.md
  → signal: ...

Convergence: spec-scope ledger has no open blocking objections and the
latest review transaction approves or approves-with-nits.
Escalation: After 5 rounds, surface unresolved objections to user
```

**User involvement (interactive mode):** User co-authors the initial
draft and can intervene between any round. In autonomous mode, Claude
writes the initial draft alone from a goal statement.

### Phase 2: Implementation

**Goal:** Produce working code that passes verification.

Runs per-pod (pods can be parallel):

```
Round 1:
  Implementer (Codex): Build from spec
  → produces: code changes + impl/status.md
  → sandbox: workspace-write

Round 2:
  Verifier (Claude): Review implementation against spec
  → produces: impl/verify-1.md
  → signal: approve | reject with objections
  → workspace: disposable verifier worktree
  → tools: can run tests/builds; source patches are discarded unless
    explicitly promoted by the dialec

Round 3+ (if rejected):
  Implementer: Fix issues
  Verifier: Re-verify

Post-convergence (impl passes verification):
  Meta-Verifier (Claude): Was the Verifier thorough enough?
  → produces: impl/meta-verify.md
  → can send Verifier back with "check X specifically"

  Deslopper (Claude): Code quality review
  → produces: impl/deslop.md
  → checks: over-abstraction, AI slop, dead code, inconsistency

  Codex reviews Meta-Verifier + Deslopper findings:
  → produces: impl/response.md
  → signal: approve | reject (defends or fixes)

  → ping pong until pod ledger has no open blocking objections across
    verifier, meta-verifier, deslopper, and implementer response scopes
```

**Pod structure:**
```
.dialec/session/phase-impl/
├── pod-auth/
│   ├── spec-slice.md        # Relevant portion of spec
│   ├── ledger.jsonl         # Pod-scoped objection ledger
│   ├── impl/
│   │   ├── round-1/        # Codex's initial implementation
│   │   ├── verify-1.md     # Claude's verification
│   │   ├── round-2/        # Codex's fixes
│   │   └── verify-2.md     # Claude's re-verification
│   ├── meta-verify.md
│   ├── deslop.md
│   └── response.md         # Codex's response to meta+deslop
└── pod-payments/
    └── ...
```

### Phase 3: Cleanup / Refactor

**Goal:** Make the code maximally maintainable for future LLM
modification. Decomposition over abstraction. No backward compat debt.

```
Round 1:
  Refactorer (Claude): Analyze implementation for LLM-maintainability
  → produces: cleanup/analysis.md
  → focus: decomposition opportunities, unclear intent, tests that
    encode invalid behavior, dead abstractions

Round 2 (interactive mode only):
  User: Clarify intent where flagged, confirm/reject "invalid test" findings
  → produces: cleanup/user-input.md

Round 3:
  Refactorer (Claude): Execute refactoring
  → produces: code changes + cleanup/changes.md

Round 4:
  Adversary (Codex): Review refactored code
  → produces: cleanup/adversarial-review.md
  → signal: approve | reject
  → focus: did the refactor break anything? did it lose intent?
    is the decomposition actually better?

Round 5+ (if rejected):
  Refactorer fixes, Adversary re-reviews, converge.
```

**User involvement is critical here.** The user must confirm:
- Intent behind ambiguous code ("is this intentional or a bug?")
- Whether flagged test cases are actually invalid
- Whether backward-compat breaks are acceptable

In autonomous mode, the Refactorer is more conservative — only makes
changes where intent is unambiguous.

## Phase Gates

Every phase transition is a dialec-owned transaction. A phase cannot
advance because an agent says it is done; it advances only when its gate is
satisfied.

### Spec Gate

Inputs:

- User goal and clarifications.
- Final spec draft.
- Full spec-review ledger.

Checks:

- No open blocking spec objections.
- Spec names acceptance criteria and non-goals.
- Spec defines verification commands or explicitly says they are unknown.
- User approval is recorded in interactive mode.

Outputs:

- Frozen spec artifact with content hash.
- Optional pod slicing plan.

### Implementation Gate

Inputs:

- Frozen spec artifact.
- Pod patches.
- Verifier, meta-verifier, deslopper, and implementer-response ledgers.

Checks:

- No open blocking implementation objections.
- All required verification commands have passing transactions.
- Integration branch applies every accepted patch cleanly.
- Final integrated diff is reviewed against the frozen spec.
- Dirty generated files are either accepted artifacts or removed.

Outputs:

- Integrated implementation patch.
- Test/build report.
- Remaining non-blocking nits ledger.

### Cleanup Gate

Inputs:

- Integrated implementation.
- Cleanup analysis.
- User clarifications for ambiguous intent.
- Cleanup adversarial-review ledger.

Checks:

- No open blocking cleanup objections.
- No accepted behavior was removed without user sign-off.
- No backward-compat break was introduced without user sign-off.
- Verification commands pass after refactor.

Outputs:

- Final patch.
- Final session report.
- Residual risks and deferred items.

## Convergence Engine

### Signal Extraction

Every agent turn must produce a structured convergence signal alongside
its natural language output. Enforced via `--json-schema` / `--output-schema`:

```json
{
  "$schema": "http://json-schema.org/draft-07/schema#",
  "type": "object",
  "required": [
    "verdict",
    "summary",
    "objections",
    "resolvedObjectionIds",
    "newObjectionIds"
  ],
  "properties": {
    "verdict": {
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
        "required": [
          "id",
          "category",
          "severity",
          "description",
          "blocking",
          "evidence",
          "status"
        ],
        "properties": {
          "id": { "type": "string" },
          "category": {
            "enum": ["correctness", "completeness", "clarity",
                     "architecture", "intent-mismatch", "style",
                     "security", "test-coverage", "operability",
                     "performance"]
          },
          "severity": {
            "enum": ["blocker", "major", "minor", "nit"]
          },
          "description": { "type": "string" },
          "blocking": { "type": "boolean" },
          "evidence": { "type": "string" },
          "proposedResolution": { "type": "string" },
          "location": { "type": "string" },
          "owner": { "type": "string" },
          "status": {
            "enum": ["open", "addressed", "withdrawn", "user-accepted"]
          }
        }
      }
    }
  }
}
```

### Objection Ledger

Convergence is evaluated against the session's objection ledger, not only
against the latest reviewer message.

```
.dialec/session/objections.jsonl
```

Each ledger entry records:

- Objection id, category, severity, blocking flag, and owner.
- First transaction that raised it.
- Evidence and location.
- Transactions that attempted to address it.
- Reviewer disposition after each attempt.
- User overrides, if any, with reason and timestamp.

Stable IDs are required. If an agent drops an objection without listing it
in `resolvedObjectionIds`, the dialec keeps it open. If an agent renames
an objection, the dialec treats the new id as a new objection unless the
signal explicitly links it to a replaced id.

### Convergence Rules

```
openBlocking(ledger, scope) :=
  ledger.entries
    .filter(o => o.scope == scope)
    .filter(o => o.status == 'open')
    .filter(o => o.blocking || o.severity in ['blocker', 'major'])

converged(signal, ledger, scope) :=
  latest transaction succeeded
  AND no schema validation errors
  AND (
    signal.verdict == 'approve'
    OR signal.verdict == 'approve-with-nits'
  )
  AND openBlocking(ledger, scope).length == 0

deadlocked(round, maxRounds, ledger, scope) :=
  round >= maxRounds
  AND openBlocking(ledger, scope).length > 0
```

### Escalation

When deadlocked:
- **Interactive mode:** Surface the remaining blocking objections to the
  user. User decides: accept, reject, or force another round.
- **Autonomous mode:** Fail closed for correctness, security, data-loss,
  migration, operability, and test-coverage blockers. For product/style
  disputes, the configured owner can win only if the dissent is logged and
  no higher-severity objection remains. The dialec may narrow scope,
  invoke an arbiter, or pause for user review.

### Arbiter (optional)

If objections are ambiguous (is this really blocking?), the dialec
can optionally invoke a fast, cheap model (Haiku) with the full context:

```
Given this artifact and this review, are the remaining objections
genuinely blocking (would cause correctness issues if ignored) or
are they preferences/style?
```

This is opt-in and only fires when the ledger has unresolved objections
whose severity/blocking status is ambiguous. Arbiter output is another
ledger entry; it does not mutate code or silently override reviewers.

## Modes

Three modes, each defining WHO drives the loop and WHERE intelligence
lives:

### Sidecar (default interactive)

The user stays in a live Claude Code session. Dialec is a tool Claude
calls to ferry work to other harnesses. The user never leaves their
Claude terminal.

```
┌────────────────────────────────────────────────────────┐
│  User's Claude Code session (interactive, persistent)  │
│                                                        │
│  User + Claude co-author spec                          │
│  Claude calls: dialec run --harness codex ...          │
│  Claude reads result, presents to user                 │
│  User + Claude iterate                                 │
│  Claude calls dialec again when ready                  │
│  ...until convergence                                  │
└────────────────────────────────────────────────────────┘
         │                          ▲
         │ dialec run               │ transaction result
         ▼                          │
┌────────────────────────────────────────────────────────┐
│  Background harnesses (headless, managed by dialec)    │
│                                                        │
│  Codex: reviews, implements, adversarial passes        │
│  Gemini: grounding, alternative perspectives           │
│  Cursor: implementation, validation                    │
└────────────────────────────────────────────────────────┘
```

**How it works:**

1. User activates a Claude Code skill (`/dialec` or similar).
2. The skill gives Claude access to dialec as a bash tool.
3. Claude acts as the spec-writer, verifier, refactorer, etc. —
   the "thinking" roles stay in the live session.
4. When Claude needs an adversarial review, implementation, or second
   opinion, it calls `dialec run` to dispatch to another harness.
5. Claude reads the transaction result (signal, objections, artifacts)
   and integrates it into the conversation.
6. The user participates naturally — they see what Codex said, they
   can agree/disagree, ask Claude to push back, etc.

**The user's live Claude session IS the coordinator.** Dialec is just
the bridge that handles process spawning, transaction recording,
worktree management, and signal extraction. Claude does the thinking
about what to send, when, and how to respond to what comes back.

**Skill integration:**

```markdown
# /dialec skill

You have access to `dialec` for multi-harness orchestration. Use it to
dispatch work to other agents (Codex, Gemini, Cursor) and read their
structured responses.

## Available commands:

- `dialec run --harness codex --role spec-reviewer --task "..." --artifact path`
  Dispatch a task to another harness. Returns transaction with signal.

- `dialec status` — Check session state
- `dialec log` — View turn history
- `dialec worktree create <name>` — Create isolated workspace for a pod
- `dialec advance --reason "..."` — Force past a deadlock
- `dialec check` — Verify harness availability

## Workflow:

1. Co-author the spec with the user.
2. When ready for adversarial review, call `dialec run` with the
   spec artifact.
3. Read Codex's signal. Present objections to the user.
4. Address objections together with the user.
5. Send revised spec back for review.
6. Repeat until convergence (no blocking objections).
7. Move to implementation: create worktrees, dispatch to Codex.
8. Verify implementations, run meta-verification, deslopping.
9. Merge converged pods.
10. Run cleanup/refactor phase.

## Convergence:

After each `dialec run`, check `.dialec/session/objections.jsonl` for
open blocking objections. Converged = no open blockers + latest verdict
is approve or approve-with-nits.
```

**Key property:** The user gets the full Claude Code interactive
experience. They can ask questions, change direction, explore tangents.
When they're ready to involve other agents, Claude handles the dispatch.
The user never needs to context-switch to another terminal or tool.

### Autonomous (headless, hackathon mode)

Claude drives end-to-end WITHOUT a live user session. A headless Claude
instance acts as the coordinator, calling dialec for all cross-harness
work and making convergence decisions autonomously.

```
┌────────────────────────────────────────────────────────┐
│  Headless Claude (coordinator, spawned by dialec)      │
│                                                        │
│  System prompt: coordinator skill                      │
│  Writes spec from goal → calls dialec for review       │
│  Reads signals → iterates → advances phases            │
│  Creates worktrees → dispatches implementation         │
│  Verifies → deslopps → merges                          │
│  Runs cleanup → final integration                      │
└────────────────────────────────────────────────────────┘
         │                          ▲
         │ dialec run               │ transaction result
         ▼                          │
┌────────────────────────────────────────────────────────┐
│  Background harnesses (headless, managed by dialec)    │
│                                                        │
│  Codex: implements, adversarial reviews                │
│  (Other harnesses as configured)                       │
└────────────────────────────────────────────────────────┘
```

**How it works:**

1. User runs: `dialec start --mode autonomous --goal "implement X"`
2. Dialec spawns a headless Claude with the coordinator skill:
   ```bash
   claude -p "$COORDINATOR_PROMPT" \
     --bare \
     --append-system-prompt-file .dialec/roles/coordinator.md \
     --allowedTools "Bash,Read,Glob,Grep,Edit,Write" \
     --max-budget-usd 8.00 \
     --max-turns 100
   ```
3. The headless Claude reads the goal, writes a spec, and calls
   `dialec run` (via Bash tool) to send it for review.
4. Claude reads the review transaction, addresses objections, sends
   revised spec.
5. On convergence, Claude advances to impl phase: slices spec into
   pods, creates worktrees, dispatches implementations.
6. Claude verifies each pod, runs meta-verification, deslopping.
7. On pod convergence, Claude merges via `dialec worktree` commands.
8. Claude runs cleanup/refactor, dispatches adversarial review.
9. Final integration, session report.

**Convergence rules in autonomous mode:**
- Stricter max rounds (3 instead of 5)
- Fail-closed on correctness/security blockers (halts, doesn't force-advance)
- Style/architecture disputes: coordinator wins after 2 rounds if the
  dispute is logged
- Budget-capped (time or cost, whichever hits first)
- On true deadlock: writes `.dialec/session/escalation.md` and exits
  with non-zero status

**The coordinator skill** is a role prompt that tells headless Claude:
- You are the coordinator for a dialec session
- You have access to `dialec run`, `dialec worktree`, `dialec status`
- Follow the phase protocol (spec → impl → cleanup)
- Check convergence after each review round
- Advance phases when gates are satisfied
- Never force-advance on correctness blockers
- Write a session report when done

**Invoking autonomous mode:**
```bash
# Time-budgeted
dialec start --mode autonomous --goal "implement auth for veriscope" --budget 4h

# Cost-budgeted
dialec start --mode autonomous --goal "refactor the compiler frontend" --budget '$10'

# Unbounded (runs until done or deadlocked)
dialec start --mode autonomous --goal "fix all lint warnings"
```

### Switching

The user can start autonomous and grab the wheel:
```bash
dialec start --mode autonomous --goal "implement auth" --budget 4h
# ... later ...
dialec intervene    # Kills coordinator, drops to sidecar mode
                    # User's next Claude session picks up where coordinator left off
```

Or start in sidecar and hand off:
```bash
# In Claude session:
> "I'm happy with the spec, let dialec run the rest autonomously"
# Claude calls:
dialec release      # Spawns headless coordinator to continue from current phase
```

**Continuity on switch:** When switching from autonomous → sidecar,
the user's Claude session reads the session state, artifact chain, and
objection ledger from `.dialec/`. Everything the coordinator did is
recorded as transactions — the user can review and continue from any
point. When switching from sidecar → autonomous, dialec spawns the
coordinator with the full session context.

### Legacy interactive (manual dispatch)

For debugging or unusual workflows, the user can still manually drive
dialec from their shell without a Claude session:

```bash
dialec start --mode interactive
dialec run --harness codex --role spec-reviewer --task "review this" --artifact spec.md
dialec status
dialec advance --reason "shipping it"
```

This is the "bag of primitives" mode. Useful for scripting or when the
user wants to be the coordinator directly.

## Session Structure

```
.dialec/
├── dialec.json              # Session state (phase, round, mode)
├── config.json                 # Harness config, convergence params
├── signal-schema.json          # Shared JSON Schema for convergence signals
├── capabilities/
│   ├── claude.json
│   ├── codex.json
│   ├── gemini.json
│   └── cursor.json
├── roles/
│   ├── coordinator.md          # System prompt for autonomous coordinator
│   ├── spec-writer.md          # System prompt for spec author
│   ├── spec-reviewer.md        # System prompt for adversarial spec reviewer
│   ├── implementer.md          # System prompt for implementer
│   ├── verifier.md             # System prompt for verifier
│   ├── meta-verifier.md        # System prompt for meta-verifier
│   ├── deslopper.md            # System prompt for deslopper
│   ├── refactorer.md           # System prompt for refactorer
│   └── adversary.md            # System prompt for adversarial cleanup reviewer
├── skills/
│   ├── sidecar.md              # Claude Code skill for sidecar mode
│   └── coordinator.md          # Claude Code skill for autonomous mode
├── session/
│   ├── objections.jsonl        # Global objection ledger
│   ├── turns/
│   │   ├── 0001-claude-spec-writer/
│   │   ├── 0002-codex-spec-reviewer/
│   │   └── ...
│   ├── integrations/
│   │   ├── pod-auth-merge/
│   │   └── final-cleanup-merge/
│   ├── phase-spec/
│   │   ├── draft-1.md
│   │   ├── review-1.json       # Includes convergence signal
│   │   ├── draft-2.md
│   │   └── review-2.json       # verdict: approve
│   ├── phase-impl/
│   │   ├── pod-auth/
│   │   │   └── ...
│   │   └── pod-payments/
│   │       └── ...
│   └── phase-cleanup/
│       ├── analysis.md
│       ├── user-input.md
│       ├── changes.md
│       └── adversarial-review.json
├── workspaces/                 # Git worktrees or workspace refs
│   ├── pod-auth-impl/
│   ├── pod-auth-verify/
│   └── ...
└── log/
    ├── costs.jsonl             # Per-turn cost tracking
    ├── decisions.jsonl         # Convergence decisions with rationale
    └── timeline.jsonl          # Phase/round transitions with timestamps
```

**dialec.json:**
```json
{
  "sessionId": "a1b2c3",
  "mode": "interactive",
  "startedAt": "2026-05-05T22:00:00Z",
  "currentPhase": "implement",
  "phases": {
    "spec": { "status": "converged", "rounds": 3, "cost": 0.12 },
    "implement": {
      "status": "in-progress",
      "pods": {
        "auth": { "status": "verifying", "round": 2 },
        "payments": { "status": "implementing", "round": 1 }
      }
    },
    "cleanup": { "status": "pending" }
  },
  "totalCost": 0.47,
  "budget": { "maxUsd": 10.0, "maxHours": null }
}
```

## Configuration

**config.json:**
```json
{
  "harnesses": {
    "claude": {
      "commandCandidates": ["claude"],
      "probe": ["--help", "--version"],
      "defaults": {
        "headless": ["-p"],
        "outputFormat": ["--output-format", "json"],
        "systemPrompt": ["--append-system-prompt-file"],
        "schema": ["--json-schema"],
        "cwd": null,
        "extraFlags": ["--bare"]
      }
    },
    "codex": {
      "commandCandidates": ["codex"],
      "probe": ["--help", "--version", "exec --help", "debug prompt-input"],
      "defaults": {
        "headless": ["exec"],
        "outputFormat": ["--json"],
        "schema": ["--output-schema"],
        "cwd": ["--cd"],
        "sandbox": ["--sandbox"],
        "approval": ["--ask-for-approval", "never"],
        "finalMessage": ["-o"]
      }
    },
    "gemini": {
      "commandCandidates": ["gemini"],
      "probe": ["--help", "--version"],
      "defaults": {
        "headless": ["-p"],
        "outputFormat": ["--output-format", "json"],
        "approval": ["--approval-mode", "yolo"]
      }
    },
    "cursor": {
      "commandCandidates": ["cursor-agent"],
      "probe": ["--help", "--version"],
      "defaults": {
        "headless": ["-p"],
        "outputFormat": ["--output-format", "json"],
        "approval": ["--force"]
      }
    }
  },
  "roles": {
    "spec-writer": "claude",
    "spec-reviewer": "codex",
    "implementer": "codex",
    "verifier": "claude",
    "meta-verifier": "claude",
    "deslopper": "claude",
    "refactorer": "claude",
    "adversary": "codex"
  },
  "convergence": {
    "maxRounds": 5,
    "useArbiter": false,
    "arbiterModel": "haiku",
    "autoAdvanceOnNits": true,
    "failClosedCategories": [
      "correctness",
      "security",
      "intent-mismatch",
      "test-coverage",
      "operability"
    ]
  },
  "workspaces": {
    "strategy": "git-worktree",
    "baseBranch": "current",
    "keepFailedWorkspaces": true,
    "dirtyUserWorkspacePolicy": "refuse"
  },
  "budget": {
    "maxUsd": 10.0,
    "perTurnMaxUsd": 2.0,
    "perPhaseMaxUsd": null
  }
}
```

Role-to-harness mapping is configurable, but not unconstrained. The
dialec only accepts a mapping if the probed harness satisfies the role's
required capabilities. Want Gemini as the implementer? Change one line, then
`dialec check` must prove Gemini can run in an isolated write workspace,
emit parseable output, and produce a patch transaction.

### Role Capability Requirements

```
spec-writer:
  requires: [headless, structured-output, artifact-read, artifact-write]

spec-reviewer:
  requires: [headless, structured-output, artifact-read]

implementer:
  requires: [headless, isolated-workspace-write, patch-extraction,
             structured-output, shell-tools]

verifier:
  requires: [headless, disposable-workspace-write, test-command-execution,
             structured-output, artifact-read]

meta-verifier:
  requires: [headless, structured-output, artifact-read]

deslopper:
  requires: [headless, structured-output, artifact-read, diff-read]

refactorer:
  requires: [headless, isolated-workspace-write, patch-extraction,
             structured-output, shell-tools]

adversary:
  requires: [headless, disposable-workspace-write, test-command-execution,
             structured-output, diff-read]
```

## CLI Interface

```bash
# Start a new session
dialec start [--mode interactive|autonomous] [--goal "..."] [--budget 4h|$10]

# Resume a session
dialec resume [session-id]

# Check status
dialec status

# Intervene in autonomous mode
dialec intervene

# Release to autonomous mode
dialec release

# Force advance past a deadlock
dialec advance [--reason "user decision: ship it"]

# Force retry current round
dialec retry [--hint "focus on X"]

# View session log
dialec log [--phase spec|impl|cleanup] [--pod auth]

# List available harnesses
dialec harnesses

# Validate config
dialec check
```

## Implementation Plan

This is a full implementation plan, not a proof-of-concept ladder. Each
milestone preserves the final architecture: capability discovery,
transaction logs, worktree isolation, objection ledger, and phase gates are
present from the first runnable build.

### Milestone 1 — Core Kernel (done — Codex built this)

- Rust binary, single crate.
- Config loader and validator.
- Capability probe framework for all configured harnesses.
- Session directory creation with immutable turn directories.
- `RunTransaction` persistence, including stdout/stderr capture, command
  metadata, before/after workspace snapshots, and patch extraction.
- Shared signal schema validation.
- Objection ledger creation and update rules.
- Dialec-owned git worktree manager.
- CLI: `start`, `status`, `check`, `log`, `run`, `worktree`.

### Milestone 2 — Sidecar Mode + Skill

- Claude Code skill (`sidecar.md`) that teaches Claude how to use
  `dialec run` as a bridge to other harnesses.
- Skill includes: convergence checking logic, phase protocol, when to
  dispatch, how to read signals, how to present objections to user.
- `dialec run` verified end-to-end with Claude + Codex (spec review
  round-trip in a live Claude session).
- Role prompts fleshed out with real instructions (not stubs).
- Artifact chain: Claude writes spec, calls dialec to send to Codex,
  reads structured signal, presents to user.

### Milestone 3 — Autonomous Mode + Phase Runner

- Coordinator skill/role prompt for headless Claude.
- `dialec start --mode autonomous` spawns headless Claude with
  coordinator prompt and full tool access.
- Phase runner logic lives in the coordinator prompt (Claude drives the
  loop, not Rust code) — dialec stays dumb.
- Convergence loop: coordinator checks ledger, decides next action.
- Phase advancement: coordinator transitions spec → impl → cleanup.
- Pod slicing: coordinator decomposes spec into pods.
- `dialec release` spawns coordinator from sidecar mode.
- `dialec intervene` kills coordinator, preserves session state for
  sidecar pickup.

### Milestone 4 — Full Role Topology

- Parallel pod execution (coordinator spawns multiple `dialec run`
  in parallel via background processes or sequential dispatch).
- Verifier, meta-verifier, deslopper convergence loop.
- Merge queue for converged pod patches.
- Merge conflict detection and escalation.
- Final integrated implementation gate.
- Memory system: gated writes after phase convergence.

### Milestone 5 — Additional Harnesses + Polish

- Gemini adapter behind capability probe.
- Cursor adapter using `cursor-agent`.
- Harness-specific event parsers.
- Arbiter for ambiguous ledger severity.
- Budget enforcement (time, cost, turns).
- Streaming observation (tail transaction in progress).
- Claudish adapter as fallback for models without native CLIs.
- Phase DAG for custom workflows beyond spec/impl/cleanup.

## Decisions

1. **Role mapping (confirmed):** Claude specs → Codex implements →
   Claude verifies → Claude refactors → Codex verifies refactors.
   Claude is the thinker/critic, Codex is the builder/adversary.

2. **Workspace isolation: dialec-managed worktrees.** The dialec owns
   worktree lifecycle for every harness. Built-in harness worktree features
   may be used only after probing, and only as an implementation detail.
   The dialec creates workspaces before spawning pod agents, passes cwd
   through the harness's supported mechanism, and handles merge/cleanup
   after convergence. See "Worktree Management" section below.

3. **Session continuity: artifact-first, resume-when-proven.** The artifact
   chain and ledger are the contract. Harness resume is used only when
   capability probes and transaction records show it is available. If resume
   fails or is unsupported, the dialec replays the relevant artifact
   chain into a fresh run.

4. **Language: Rust.** Fast, single-binary distribution, good process
   management, no runtime deps. No TypeScript fallback in the core.

## Worktree Management

The dialec, not individual harnesses, owns git worktree lifecycle.
Harness-native workspace features are treated as transport mechanisms, not
the source of truth.

### Lifecycle

```
1. Dialec creates worktree:
   git worktree add .dialec/workspaces/pod-{name} -b dialec/{name}

2. Dialec spawns implementer in the worktree directory:
   codex exec --cd .dialec/workspaces/pod-auth "implement auth per spec"

3. Dialec creates a disposable verifier worktree from the pod branch:
   git worktree add .dialec/workspaces/pod-auth-verify dialec/auth

4. Verifier runs in the disposable worktree:
   claude -p "verify implementation" --add-dir .dialec/workspaces/pod-auth-verify

5. On convergence, dialec merges:
   cd .dialec/workspaces/pod-auth
   git add -A && git commit -m "pod-auth: implement auth"
   cd $PROJECT_ROOT
   git merge dialec/auth --no-ff

6. Dialec cleans up:
   git worktree remove .dialec/workspaces/pod-auth
   git worktree remove .dialec/workspaces/pod-auth-verify
   git branch -d dialec/auth
```

### Parallel Pods

Each pod gets its own worktree branched from the same base commit.
Pods run in parallel. Merge conflicts between pods are resolved by the
dialec (or escalated to user in interactive mode).

```
main ─────┬──────────────────────────────── merge pod-auth ── merge pod-payments
          │                                       ↑                  ↑
          ├── dialec/auth ─── impl ─── ✓ ─────┘                  │
          │                                                         │
          └── dialec/payments ─── impl ─── ✓ ───────────────────┘
```

### Harness-Specific Worktree Behavior

| Harness | How it enters the worktree |
|---------|---------------------------|
| Claude  | `--add-dir` for extra read roots; otherwise run with cwd set to worktree |
| Codex   | `codex exec --cd <worktree-path>` + `--sandbox workspace-write` |
| Gemini  | Run with cwd set to worktree path, plus any probed include-dir flags |
| Cursor  | `cursor-agent` with cwd set to worktree path |

The dialec `chdir`s or passes `--cd` / `--add-dir` as appropriate.
Agents see a normal git repo. They do not own branch creation, merge,
cleanup, or integration policy.

## Memory

Agents maintain two levels of context:

### 1. Artifact Chain + Optional Session Resume

Each round receives the relevant artifact chain and open objection ledger.
If a harness has proven resume support, the adapter may also resume its
previous session:

- Claude: `--resume <session-id>` when available.
- Codex: `codex exec resume` / session id support when available.
- Gemini: only if the local CLI probe proves resume support.
- Cursor: only if the local CLI probe proves resume support.

Resume is a cache of conversation context, not the system of record. If
resume fails, the dialec starts fresh and replays the artifact chain.

### 2. Persistent Memory (across phases and sessions)

A shared memory store that all agents can read and (optionally) write:

```
.dialec/memory/
├── project.md          # What this project is, its architecture
├── decisions.md        # Key decisions made during spec/impl
├── patterns.md         # Code patterns established, naming conventions
├── gotchas.md          # Things that went wrong, traps to avoid
└── user-prefs.md       # User's stated preferences (from interactive rounds)
```

**How agents access memory:**
- Memory files are injected into the system prompt or prepended to the
  task prompt as context.
- After each converged phase, the dialec (or a designated agent)
  updates memory with learnings from that phase.
- Memory is append-friendly — entries have timestamps and phase tags.

**Memory writes are gated:** Only the dialec (or a designated
"memory curator" agent) writes to memory. Individual agents can
propose memory updates as part of their output, but the dialec
decides what actually persists. This prevents memory pollution from
a single agent's bad take.

**Cross-session memory:** When `dialec start` is run on a project
that already has `.dialec/memory/`, it picks up where it left off.
New sessions inherit the full memory of prior sessions.

### 3. Per-Agent Scratchpad (ephemeral)

Each agent can write to a per-agent scratch directory during its turn:

```
.dialec/scratch/verifier/
├── notes.md
└── findings/
```

This is NOT shared with other agents — it's private working space that
persists across rounds as session data but is cleaned up when the phase
ends. Useful for the verifier to track what it's already checked, or the
deslopper to maintain a running list of issues.

## Open Questions (Remaining)

1. **Role prompt injection per harness.** Codex currently discovers
   `AGENTS.md` in local prompt probes; Claude has explicit system-prompt
   flags; Gemini and Cursor use their own project-context mechanisms. The
   dialec should prefer explicit prompt flags when available and use
   temporary worktree-local instruction files only when probes confirm the
   harness reads them.

2. **Streaming intervention policy.** Streaming is required for
   observation and cancellation, but the dialec should be cautious about
   mid-run intervention. Interrupting an agent can leave a partial patch;
   that must become a failed transaction with before/after state captured.

3. **Merge conflict resolution.** When parallel pods produce
   conflicting changes, who resolves? Options: (a) Claude as a merge
   agent; (b) escalate to user; (c) sequential merge with second pod
   rebasing on first. Recommendation: (c) by default, (b) if rebase
   fails.

4. **Memory curator.** Should memory updates be: (a) automatic after
   each phase (dialec extracts key decisions); (b) a dedicated
   agent that reviews the phase and distills learnings; (c) user-
   curated in interactive mode. Probably (a) for autonomous, (c) for
   interactive, with (b) as a future refinement.

## Why Rust

- **Single binary.** No runtime, no node_modules, no venv. `cargo
  install dialec` or download a binary.
- **Process management is Rust's sweet spot.** Spawning CLIs, piping
  stdio, managing parallel worktree operations, handling signals —
  tokio + Command is built for this.
- **Fast startup.** The dialec is invoked frequently (every round).
  It should be instant, not "wait for node to boot."
- **Keep the core in Rust.** Complex JSON schema handling, event parsing,
  and process orchestration stay in-process unless a deliberately isolated
  helper is justified later. The dialec should not depend on a second
  runtime for correctness.

## Relationship to Existing Work

- **hackathon.md** — The dialec subsumes the hackathon skill. The
  hackathon skill becomes a preset configuration for the dialec:
  all roles mapped to Claude, autonomous mode, time-budgeted.
- **claudish** — Complementary, not competing. Claudish is useful if
  you want to use a model that doesn't have a native CLI (e.g., local
  Ollama, DeepSeek). The dialec can have a "claudish" adapter as a
  fallback harness for models without native CLIs.
- **veriscope/stipulate** — These are target projects the dialec
  would be used ON, not components of the dialec itself. Though
  the verification philosophy (assertions > test cases, convergence >
  sign-off) informs the dialec's convergence design.
