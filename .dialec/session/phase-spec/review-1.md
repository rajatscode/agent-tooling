# Spec Review: Rust CLI Todo App

**Reviewer:** spec-reviewer  
**Date:** 2026-05-07  
**Draft:** draft-1.md  
**Verdict:** **REJECTED** — Correctness and clarity blockers require resolution.

---

## Executive Summary

The specification covers the core requirements in the user goal (Rust CLI, JSON storage, add/list/delete commands) but contains **ambiguities and contradictions** that would cause implementation confusion and potential feature misalignment. Two critical issues must be resolved before proceeding.

---

## Critical Blockers

### 1. **Delete Command Behavior is Ambiguous** ⛔

**Problem:**
The spec states:
> "Remove todo by ID or mark as complete (implementation choice)"

This creates two unacceptable problems:
- **Specification ambiguity:** The spec should specify one clear behavior, not leave it as "implementation choice" at spec time. This is a spec defect, not an implementation flexibility.
- **Intent mismatch with goal:** The user goal says "delete commands" — implying removal. The spec allows for "mark as complete" instead, which is a different operation. The data model includes a `completed` field, suggesting completed todos persist in the file, but "delete" typically means removal.

**Current inconsistencies:**
- Section 3 says "Remove todo by ID **or** mark as complete"
- Example shows `todo delete 1` → "Deleted todo #1" (removal language)
- But the data model includes a `completed` field (suggesting soft delete is planned)
- The list command shows `[x] 2. Review PR` (completed status display), which implies completed todos stay in the list

**Required clarification:**
Choose one and commit:
- **Option A:** Hard delete (remove from JSON immediately, no `completed` field needed)
- **Option B:** Soft delete (mark `completed: true`, keep in file, don't remove)

Both are valid, but the spec cannot be vague. This will cause the implementer to guess and potentially build the wrong behavior.

---

### 2. **CLI Argument Parsing Strategy Underspecified** ⛔

**Problem:**
The spec says:
> "Use `std::env::args()` or lightweight crate (e.g., `clap` optional)"

Issues:
- **`std::env::args()` is fragile:** It doesn't handle quoted arguments or escaping well. `todo add "buy milk and bread"` could break.
- **This is not truly optional:** For a CLI tool, proper argument parsing is a core requirement, not a "nice-to-have UX improvement."
- **Vagueness invites incorrect decisions:** The implementer might pick `std::env::args()` (easier, less dependencies) and ship a broken parser that doesn't handle spaces in task descriptions.

**Required clarification:**
- Specify that `clap` (or equivalent structured parser) is **required**, not optional, to properly handle quoted/escaped arguments
- OR explicitly accept and document the limitation: "Task descriptions without spaces only"

---

## Secondary Issues

### 3. **File Corruption / Recovery Not Specified**

The spec assumes the JSON file is always valid. What happens if:
- The file is corrupted (truncated, invalid JSON)?
- The file is deleted while the app is running?
- The file is manually edited to invalid JSON?

**Suggested resolution:** Add a section on recovery strategy (e.g., "Load fails gracefully; prompt to reset file" or "Backup on save").

---

### 4. **Ambiguous Delete Command Semantics in Output**

The success criteria lists:
- `[x] 2. Review PR` — but if this todo is "completed," should `todo delete 1` remove it or error (already done)?

This needs clarification in the delete command behavior.

---

## Minor Clarity Issues

1. **File Location:** The spec says "Default: `~/.todos.json` (or configurable via environment variable)" but doesn't specify the environment variable name. (e.g., `TODO_FILE`, `TODOS_PATH`?)

2. **ID Stability:** The spec says IDs are "stable and auto-increment" but doesn't specify: If you delete todo #1, then add a new one, is it #2 or #1? (This matters for persistence.)

3. **Empty Task Description:** Spec rejects empty descriptions — good. But what about whitespace-only descriptions like `"   "`?

4. **Persistence Strategy:** The spec says "Persist immediately to JSON file" but doesn't clarify whether this is atomic (write to temp file, then move) or direct (overwrite immediately). Direct writes risk corruption on crash.

---

## What the Spec Gets Right ✓

- Clear data model with JSON structure
- Sensible command interface (add, list, delete)
- User-friendly error messages and examples
- Reasonable dependencies (serde_json, optional clap)
- Success criteria are testable
- Out-of-scope clearly delineated

---

## Verdict: **REJECTED**

**Status:** Do not proceed to implementation.

**Reason:** The delete command behavior (hard delete vs. soft delete) and CLI argument parsing strategy are fundamental architectural choices that must be locked down in the spec. Implementers need unambiguous requirements.

**Required Actions Before Resubmission:**

1. **Resolve delete semantics:** Commit to either hard delete (remove from file) or soft delete (mark completed). Update:
   - Section 3 behavior description
   - Section 3 output examples  
   - Data model (remove `completed` field if hard delete, or remove ambiguity if soft delete)
   - Success criteria

2. **Require proper CLI parsing:** Change "optional" to "required" for clap or equivalent, OR explicitly limit task descriptions to single words with clear documentation of the limitation.

3. **Add file corruption recovery strategy** (brief, 2-3 sentences in a new "Robustness" section).

4. **Clarify ID persistence:** Specify whether deleted IDs are reused or skipped (e.g., next todo after deleting #1 is #2, not #1).

---

## Convergence Signal

**Ready for implementation?** No.

**Blockers resolved?** No.

**Return to spec-writer:** Request revision of draft-2.md addressing the two critical blockers above.
