# Spec Review 3: Rust CLI Todo App

**Reviewer:** spec-reviewer  
**Date:** 2026-05-07  
**Draft:** draft-3.md  
**Verdict:** **REJECTED** — One critical blocker on error handling remains.

---

## Executive Summary

Draft-3 **successfully resolves both critical blockers from review-2:**
- ✅ Delete command output format inconsistency — now clearly specifies description in output (line 56)
- ✅ Timestamp implementation guidance — `chrono` moved to required dependencies (line 218)

However, **a new critical issue has surfaced** through systematic cross-reference of error handling across features and the robustness section. The spec is incomplete and inconsistent on file I/O error handling, which directly impacts the success criterion requiring manual testing against CLI examples.

---

## Critical Blocker: File I/O Error Handling Incomplete & Inconsistent ⛔

**Problem:**

The spec specifies what errors should be shown in the CLI examples and Robustness section, but individual feature sections do not consistently implement this:

### Issue 1: List Command Error Handling Incorrect

**Feature section (line 48):**
```
**Error Handling:** Display "Error: Could not read todos" if file is missing or corrupted
```

**Robustness section (lines 146-149):**
```
**File doesn't exist on first read:** Treat as empty list; proceed normally
**File exists but is invalid JSON:** Display "Error: Todos file is corrupted. Please check `~/.todos.json` or delete it to reset."
```

**CLI examples (lines 206-207):**
```bash
$ todo list
Error: Todos file is corrupted. Please check ~/.todos.json or delete it to reset.
```

**Contradiction:** Line 48 says show "Error: Could not read todos" if file is missing, but the Robustness section explicitly says to treat missing files as empty, not error. The CLI examples show the robustness version error message, not the feature section message. An implementer following line 48 might show the wrong error message and fail manual testing.

### Issue 2: Delete Command Error Handling Incomplete

**Feature section (lines 57-60):**
```
**Error Handling:** 
  - If ID not found: "Error: Todo #<id> not found"
  - If ID is invalid (not a number): "Error: Invalid ID. Expected a number."
  - If file write fails: "Error: Could not save todos"
```

**Robustness section (line 147):**
```
On `list` or `delete`: Display "Error: Todos file is corrupted. Please check `~/.todos.json` or delete it to reset."
```

**Missing error case:** Delete command must load the file to find the todo, so it can fail with file corruption. But the feature section doesn't mention this error case. The robustness section specifies that delete should handle it, but it's not in the feature spec.

### Issue 3: Add Command Error Handling Incomplete

**Feature section (lines 31-33):**
```
**Error Handling:** 
  - Reject empty descriptions or whitespace-only strings with message: "Error: Task description cannot be empty"
  - Reject if todo file cannot be written (disk full, permissions, etc.)
```

**Robustness section (line 148-149):**
```
On `add`: Attempt to load and fail gracefully; offer same error message [the file corruption message]
```

**Missing error case:** Add command must load the file to determine the next ID, so it can fail with file corruption. But the feature section only mentions empty description and write failure. The robustness section specifies file corruption handling for add.

---

## Why This Is a Critical Blocker

The success criteria (line 244) states:
> "Manual testing confirms all examples in 'CLI Behavior & Examples' section work as shown"

The CLI examples include the file corruption error message (lines 206-207). But:
- The List command feature section references a **different** error message ("Error: Could not read todos")
- The Delete and Add feature sections **omit** file corruption error handling entirely

An implementer reading only the feature sections would miss file corruption handling for delete and add, and use the wrong error message for list. This would cause manual testing failures.

---

## Secondary Issues (Non-Blocking)

### 1. Delete Command Alias Ambiguity

**Line 51** (Delete Todo section):
> "Command: `todo delete <id>` or `todo rm <id>` (both acceptable)"

**Line 210** (Alternative Command Aliases section):
> "`todo rm <id>` as alias for `todo delete <id>` — acceptable but not required"

**Inconsistency:** The feature section describes `rm` as "both acceptable" (implying required), but the alternatives section says it's "optional, not required." The CLI examples only show the `delete` command, not `rm`. Should an implementer implement `rm` or not?

**Impact:** Low. The spec explicitly marks it as "not required," so skipping it is acceptable. But the wording in the feature section creates doubt. Could be clarified to say `rm` is optional.

### 2. Delete Method Signature vs. Output Requirement

**Architecture section (line 116):**
```rust
delete(id: u32) -> Result<(), Error>
```

**Delete feature section (line 56):**
```
Output: Confirmation with description (e.g., "Deleted todo #1: Buy groceries")
```

**Mismatch:** The method returns `Result<(), Error>` (nothing), but the output must include the deleted todo's description. The implementer will need to return `Result<Todo, Error>` or `Result<String, Error>` instead.

**Impact:** Low. This is a minor method signature detail that doesn't prevent implementation. The implementer will naturally adjust to return the description.

### 3. File Not Found vs. File Corruption Distinction

**Robustness section (line 146):**
> "File doesn't exist on first read: Treat as empty list; proceed normally"

But the feature section (line 48) groups "missing or corrupted" together. The spec should clarify that **missing = empty list** (always, not just "first read"), and **corrupted = error message**. The current wording is ambiguous about whether a missing file (after todos have been added) is treated as empty or error.

**Impact:** Low. The intent is clear from the robustness section, but the wording could be tighter.

### 4. Whitespace Trimming in Descriptions

The spec rejects "whitespace-only descriptions" (line 237) but doesn't clarify whether leading/trailing whitespace should be trimmed. For example:
```bash
$ todo add "  Buy groceries  "
```
Should this be stored as `"  Buy groceries  "` (with spaces) or `"Buy groceries"` (trimmed)? The examples show no extra spaces, but explicit guidance would help.

**Impact:** Very Low. Implementation detail.

---

## Verification: Draft-2 Blockers Remain Resolved ✓

| Issue | Review-2 Status | Draft-3 Status | Evidence |
|-------|---|---|---|
| Delete output format inconsistency | ❌ Blocker | ✅ Resolved | Line 56: Output description now explicit |
| Timestamp implementation guidance | ❌ Blocker | ✅ Resolved | Line 218: chrono in required dependencies |

Both prior blockers remain fully resolved. Draft-3 introduces a **new blocker** through incomplete feature-level error specification.

---

## What Draft-3 Gets Right ✓

- Clear, unambiguous delete semantics (hard delete)
- Comprehensive robustness section with detailed file handling
- Strong CLI parsing requirement (clap)
- Consistent delete output format with examples
- Clear dependencies (chrono now required)
- Testable success criteria
- Well-scoped out-of-scope section
- Delete output now includes description (resolves review-2 blocker)

---

## Verdict: **REJECTED**

**Status:** Do not proceed to implementation.

**Reason:** File I/O error handling is incompletely specified across feature sections. The Robustness section defines error behavior, but individual feature sections don't consistently require it. This will cause implementers to miss error cases and fail manual testing against CLI examples.

**Required Actions Before Resubmission:**

1. **CRITICAL — Unify Error Handling Across Features**

   Update the error handling for all three commands to explicitly reference file I/O errors:

   **List Todos (line 48), update to:**
   ```
   **Error Handling:** 
   - If file doesn't exist: treat as empty list and display empty message
   - If file exists but is invalid JSON: Display "Error: Todos file is corrupted. Please check ~/.todos.json or delete it to reset."
   ```

   **Delete Todos (after line 60), add:**
   ```
   - If JSON file is corrupted: Display "Error: Todos file is corrupted. Please check ~/.todos.json or delete it to reset." and exit with code 1
   ```

   **Add Todo (after line 33), add:**
   ```
   - If JSON file is corrupted: Display "Error: Todos file is corrupted. Please check ~/.todos.json or delete it to reset." and exit with code 1
   ```

   OR, more cleanly: Add a note at the top of each feature section:
   ```
   Note: See the Robustness section for handling of missing files and JSON corruption errors, which may occur during the file load step.
   ```

   But explicit error handling in each feature section is clearer for implementers.

2. **NICE-TO-HAVE — Clarify Delete Alias Requirement**

   Update line 51 or line 210 to remove ambiguity. Either:
   - Remove `rm` from line 51, keep it as optional-only (line 210), OR
   - Move `rm` into the required features and update success criteria

3. **NICE-TO-HAVE — Clarify "First Read" vs. Always**

   Update the Robustness section (line 146) to clarify that "file doesn't exist" means **always**, not just on first use:
   ```
   **File doesn't exist (any time):** Treat as empty list; proceed normally
   ```

---

## Convergence Signal

**Ready for implementation?** No, not yet.

**Can implementation begin after critical fix?** Yes, after unifying error handling across all three feature sections.

**Blockers resolved?** No — file I/O error handling must be explicitly required in the feature sections, not just in the robustness section.

**Return to spec-writer:** Request revision of draft-3.md to explicitly specify file corruption error handling in the List, Delete, and Add feature sections. Once error messages are consistently specified across all features and robustness section, resubmit for final approval.

**Estimated impact of fix:** Low — this is a clarification that unifies information already present in the spec. Draft-4 should resolve quickly.

