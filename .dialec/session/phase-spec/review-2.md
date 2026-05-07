# Spec Review 2: Rust CLI Todo App

**Reviewer:** spec-reviewer  
**Date:** 2026-05-07  
**Draft:** draft-2.md  
**Verdict:** **REJECTED** — One critical blocker remains; one clarity issue requires resolution.

---

## Executive Summary

Draft-2 successfully resolves **all critical blockers from draft-1:**
- ✅ Delete semantics unambiguously specified (hard delete, no `completed` field)
- ✅ Clap is now required, not optional
- ✅ Robustness section addresses file corruption
- ✅ ID stability, whitespace rejection, environment variable support all clarified

However, **a new inconsistency has been introduced** between the spec sections and the CLI examples, and one **vague implementation guidance** remains. These must be resolved before implementation.

---

## Critical Blocker: Delete Command Output Format Inconsistency ⛔

**Problem:**

The spec contains conflicting descriptions of the delete command output:

**Delete Todo section (line 56):**
```
Output: Confirmation (e.g., "Deleted todo #1" or "Removed todo #1")
```

**CLI Behavior & Examples section (line 170):**
```bash
$ todo delete 1
Deleted todo #1: Buy groceries
```

**The inconsistency:**
- The feature spec shows output WITHOUT the task description
- The usage examples show output WITH the task description

An implementer following the Delete Todo section might ship `"Deleted todo #1"` (without description). This would fail the manual test against the CLI examples.

**Why this is a blocker:**
- The spec includes success criterion: "Manual testing confirms all examples in 'CLI Behavior & Examples' section work as shown"
- The examples show description; the spec section doesn't require it
- Implementers will follow either the feature spec or the examples, and one will be wrong

**Required resolution:**
Update Delete Todo section to show:
```
Output: Confirmation with description (e.g., "Deleted todo #1: Buy groceries" or "Removed todo #1: Buy groceries")
```

---

## Secondary Issue: Timestamp Implementation Guidance is Vague

**Problem:**

The spec requires ISO 8601 timestamps in the data model:
```json
"created_at": "2026-05-07T12:30:00Z"
```

But the dependencies section contains a contradiction:
```
**Optional (nice-to-have, not required for v1):**
- `chrono` if timestamps are desired (for now, use `std::time` or a simple string format)
```

**Contradiction:**
- Timestamps ARE required (they're in the mandatory data model, not optional)
- But the deps section marks chrono as optional and says "if timestamps are desired"
- Guidance to "use `std::time`" is vague — `std::time::SystemTime` doesn't provide ISO 8601 formatting directly
- "simple string format" is unclear; does this mean the implementer should manually format?

**Why this matters:**
- An implementer might think timestamps are optional and skip them
- Or they might try to use `std::time` and fail to produce valid ISO 8601 without additional work
- The spec should be clear on the implementation path

**Required resolution:**
Choose and commit to one:

**Option A (Recommended):** Include `chrono` in required dependencies
```
**Required:**
- `serde`, `serde_json`, `clap`, `chrono` (for ISO 8601 timestamps)
```

**Option B:** Clarify the implementation path in Optional section:
```
**Optional:**
- `chrono` for convenient ISO 8601 formatting. Without it, manually format SystemTime using string interpolation.
```

Option A is cleaner and avoids implementation complexity.

---

## Verification: Draft-1 Blockers Resolution ✅

All critical and secondary issues from draft-1 have been addressed:

| Issue | Draft-1 Status | Draft-2 Status | Evidence |
|-------|---|---|---|
| Delete semantics (hard vs soft) | ❌ Ambiguous | ✅ Resolved | Section 3: "Hard delete: Remove the todo by ID from the JSON file permanently" |
| CLI parsing requirement | ❌ Optional | ✅ Required | Architecture: "**Required:** Use `clap` crate... No fallback to `std::env::args()`" |
| File corruption recovery | ❌ Not specified | ✅ Specified | New "Robustness" section with clear handling |
| Environment variable support | ❌ Vague | ✅ Specified | "Override: Via environment variable `TODO_FILE`" |
| ID persistence | ❌ Underspecified | ✅ Clear | "IDs are permanent and never reused, even after deletion" |
| Whitespace-only descriptions | ❌ Omitted | ✅ Specified | "Reject empty descriptions or whitespace-only strings" |
| Atomic write strategy | ❌ Missing | ✅ Addressed | "Write to a temporary file in the same directory, then rename" |

---

## Minor Clarity Notes (Non-Blocking)

1. **Success Criteria - "Code compiles without warnings"**
   - Acceptable, but could specify: "without clippy warnings" or "without compiler warnings" for clarity

2. **Delete Method Return Type**
   - Architecture specifies: `delete(id: u32) -> Result<(), Error>`
   - But output must include the deleted todo's description
   - Implementer will need to either:
     - Return `Result<Todo, Error>` instead
     - Return `Result<String, Error>` (description)
     - Fetch description before calling delete
   - Not a blocker, but could be clearer

3. **Version History Section**
   - Versions table is metadata appropriate for change tracking, but including it in the spec itself is unconventional
   - Consider moving to git commit messages or separate changelog

4. **Home Directory Expansion**
   - Spec says "~/.todos.json" but Rust doesn't auto-expand `~`
   - Implementation will need to use `dirs` crate or `std::env::var("HOME")`
   - Not a spec issue, but implementer should be aware

---

## What Draft-2 Gets Right ✓

- Clear, unambiguous delete semantics (hard delete)
- Strong CLI parsing requirements with rationale
- Comprehensive robustness section
- Clear error messages with examples
- Well-structured project layout
- Realistic dependencies
- Testable success criteria
- Good separation of Phase 1 scope

---

## Verdict: **REJECTED**

**Status:** Do not proceed to implementation.

**Reason:** The delete command output format is inconsistent between the feature spec and the usage examples. This will cause test failures. The timestamp implementation guidance is contradictory.

**Required Actions Before Resubmission:**

1. **CRITICAL — Fix Delete Output Inconsistency:**
   - Update Delete Todo section (line 56) to explicitly show the description in output examples
   - Ensure: "Deleted todo #1: Buy groceries" format is in the spec, not just the examples
   - Verify all error cases also match their CLI examples

2. **CRITICAL — Resolve Timestamp Implementation:**
   - Option A: Add `chrono` to required dependencies (cleanest)
   - Option B: Clarify in Optional section how to format ISO 8601 without chrono
   - Choose one approach and document clearly

3. **NICE-TO-HAVE — Clarify Delete Method Signature:**
   - Specify whether `delete()` returns the deleted Todo/description or caller must fetch first
   - Or note that this is an implementation detail left to the implementer

4. **NICE-TO-HAVE — Consider removing self-approval:**
   - The draft includes "Convergence Signal: APPROVED FOR IMPLEMENTATION"
   - Spec-writer should not self-approve; this is the reviewer's role
   - Remove this section from the draft and let the reviewer provide the signal

---

## Convergence Signal

**Ready for implementation?** No, not yet.

**Can implementation begin after fixes?** Yes, pending the two critical resolutions above.

**Blockers resolved?** No — delete output inconsistency and timestamp guidance must be addressed.

**Return to spec-writer:** Request revision of draft-2.md addressing the critical output format and timestamp guidance issues. Once resolved, resubmit for final review approval.

**Estimated impact of fixes:** Low — these are clarifications, not architectural changes. Draft-3 should resolve quickly.

