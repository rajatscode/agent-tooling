# Rust CLI Todo App Specification (Revised)

## Executive Summary

Build a lightweight command-line todo application in Rust that allows users to manage a personal task list stored persistently in JSON format. The app provides three core operations: add new todos, list existing todos, and delete todos. This is a straightforward implementation with clear, unambiguous requirements.

## Goals & Constraints

**Goals:**
- Create a simple, single-file JSON-based persistence layer
- Implement three essential todo management operations (add, list, hard delete)
- Provide clear, user-friendly CLI feedback
- Ensure reliable data persistence and recovery from file corruption

**Constraints:**
- No external database; JSON file only
- No GUI or advanced features
- Single user (no authentication/multi-user support needed)
- Minimal dependencies (serde_json + clap required)

## Core Features

### 1. Add Todo
- **Command:** `todo add <task-description>`
- **Behavior:**
  - Accept a task description as a CLI argument (may contain spaces and special characters via quoted arguments)
  - Assign a unique ID (auto-incrementing integer; next ID = max existing ID + 1)
  - Record creation timestamp (ISO 8601 format, UTC)
  - Persist immediately to JSON file
- **Output:** Confirmation with assigned ID (e.g., "Added todo #1: Buy milk")
- **Error Handling:** 
  - Reject empty descriptions or whitespace-only strings with message: "Error: Task description cannot be empty"
  - Reject if todo file cannot be written (disk full, permissions, etc.)

### 2. List Todos
- **Command:** `todo list` or `todo` (with no arguments)
- **Behavior:**
  - Display all todos in order of ID (ascending, i.e., oldest/lowest ID first)
  - Show: ID, description
  - Format each line as: `[ ] <id>. <description>` (note: square brackets are literal; no checkbox state is stored)
  - Display friendly message if list is empty: "No todos yet. Add one with: todo add \"<task>\""
- **Output Example:**
  ```
  [ ] 1. Buy groceries
  [ ] 2. Call dentist
  [ ] 3. Review PR
  ```
- **Error Handling:** Display "Error: Could not read todos" if file is missing or corrupted (see Robustness section)

### 3. Delete Todo
- **Command:** `todo delete <id>` or `todo rm <id>` (both acceptable)
- **Behavior:**
  - Hard delete: Remove the todo by ID from the JSON file permanently
  - After deletion, remaining todos keep their original IDs (no renumbering)
  - Persist changes immediately to JSON file
- **Output:** Confirmation (e.g., "Deleted todo #1" or "Removed todo #1")
- **Error Handling:** 
  - If ID not found: "Error: Todo #<id> not found"
  - If ID is invalid (not a number): "Error: Invalid ID. Expected a number."
  - If file write fails: "Error: Could not save todos"

## Data Model

### Todo Structure
```json
{
  "todos": [
    {
      "id": 1,
      "description": "Buy milk",
      "created_at": "2026-05-07T12:30:00Z"
    },
    {
      "id": 2,
      "description": "Call dentist",
      "created_at": "2026-05-07T14:15:00Z"
    }
  ]
}
```

**Key design notes:**
- No `completed` field (todos are either in the list or deleted)
- IDs are permanent and never reused, even after deletion
- `created_at` timestamp is ISO 8601 UTC; useful for auditing and future features

### File Location
- **Default:** `~/.todos.json` (i.e., `$HOME/.todos.json`)
- **Override:** Via environment variable `TODO_FILE` (if set, use this path instead of default)
- Create file automatically on first write if it doesn't exist
- If file doesn't exist on read (first use), treat as empty list
- Ensure file permissions allow user read/write (on Unix: 0600 preferred but not enforced)

## Architecture & Implementation

### Project Structure
```
src/
  main.rs          # CLI argument parsing and command dispatch
  models.rs        # Todo struct and data model
  storage.rs       # JSON file I/O operations
  commands.rs      # Add, list, delete command handlers
  error.rs         # Custom error types
```

### Key Components

1. **CLI Parser:** 
   - **Required:** Use `clap` crate for robust argument parsing
   - Rationale: Must correctly handle quoted arguments, spaces, and special characters in task descriptions (e.g., `todo add "buy milk and bread"`)
   - No fallback to `std::env::args()` — it is too fragile for production use

2. **Todo Manager / Models:** 
   - `Todo` struct with fields: `id`, `description`, `created_at`
   - `TodoList` struct that holds vec of todos and provides add/delete/list methods
   - Methods: `add(description: &str) -> Todo`, `delete(id: u32) -> Result<(), Error>`, `list() -> &[Todo]`

3. **Storage:** 
   - Load todos from JSON file on startup
   - Save todos to JSON file after each mutation (atomic write preferred; see Robustness)
   - Use `serde` and `serde_json` for serialization/deserialization

4. **Error Handling:** 
   - Define custom error enum: `TodoError { FileNotFound, InvalidJson, IoError(String), NotFound, InvalidId, EmptyDescription }`
   - Surface user-friendly messages (don't expose stack traces)

### Workflow
1. Parse command-line arguments using clap
2. Determine command (add/list/delete/help)
3. Load todos from JSON file (or initialize empty if file missing on read)
4. Execute requested operation
5. Save updated todos back to JSON file
6. Display user feedback
7. Exit with status 0 (success) or non-zero (error)

## Robustness

### File Corruption & Recovery

**Corruption Scenarios & Handling:**
- **File doesn't exist on first read:** Treat as empty list; proceed normally
- **File exists but is invalid JSON:** 
  - On `list` or `delete`: Display "Error: Todos file is corrupted. Please check `~/.todos.json` or delete it to reset." and exit with code 1
  - On `add`: Attempt to load and fail gracefully; offer same error message
  - Recovery: User can delete the file (`rm ~/.todos.json`) to start fresh

**Write Safety:**
- Use atomic writes where practical: Write to a temporary file in the same directory, then rename to `~/.todos.json`
- Rationale: If the process crashes during write, the old file remains intact instead of being left partially written
- If atomic write is infeasible due to Rust std library limitations, document this assumption and accept direct overwrites as acceptable

### Crash Scenarios
- No in-memory caching; each command loads fresh and saves immediately
- If process is killed mid-write: Recovery as per corruption section (file may be corrupted; user can reset)

## CLI Behavior & Examples

### Usage Examples
```bash
$ todo add "Buy groceries"
Added todo #1: Buy groceries

$ todo add "Call dentist"
Added todo #2: Call dentist

$ todo list
[ ] 1. Buy groceries
[ ] 2. Call dentist

$ todo delete 1
Deleted todo #1: Buy groceries

$ todo list
[ ] 2. Call dentist

$ todo add "Review PR"
Added todo #3: Review PR

$ todo
[ ] 2. Call dentist
[ ] 3. Review PR
```

### Error Cases
```bash
$ todo add ""
Error: Task description cannot be empty

$ todo add "   "
Error: Task description cannot be empty

$ todo delete 999
Error: Todo #999 not found

$ todo delete abc
Error: Invalid ID. Expected a number.

$ todo list
No todos yet. Add one with: todo add "<task>"

# (If file is corrupted)
$ todo list
Error: Todos file is corrupted. Please check ~/.todos.json or delete it to reset.
```

### Alternative Command Aliases (optional, not required)
- `todo rm <id>` as alias for `todo delete <id>` — acceptable but not required

## Dependencies

**Required:**
- `serde` and `serde_json` for JSON serialization
- `clap` for robust CLI argument parsing (non-negotiable; required in Cargo.toml)

**Standard library only:**
- `std::fs` for file I/O
- `std::env` for environment variables
- `std::path::PathBuf` for path handling

**Optional (nice-to-have, not required for v1):**
- `colored` or `termcolor` for colored output
- `chrono` if timestamps are desired (for now, use `std::time` or a simple string format)

**Initial scope:** Use `serde_json` + `clap` + standard library. Do not add optional dependencies unless explicitly requested.

## Success Criteria

- [ ] All three commands (add, list, delete) work correctly with stated behavior
- [ ] `clap` is used for CLI parsing; quoted arguments with spaces work correctly
- [ ] Data persists to JSON file between sessions
- [ ] Todo IDs are stable: auto-increment, never reused, even after deletion
- [ ] Empty descriptions and whitespace-only descriptions are rejected
- [ ] Non-existent IDs handled with clear error message
- [ ] File corruption detection and recovery strategy implemented (user can reset via deletion)
- [ ] Environment variable `TODO_FILE` is honored if set
- [ ] Code compiles without warnings
- [ ] Manual testing confirms all examples in "CLI Behavior & Examples" section work as shown

## Out of Scope (Phase 1)

- Edit/update existing todo descriptions
- Filtering, searching, or sorting by criteria other than ID
- Priority levels, categories, or tags
- Due dates or reminders
- Multi-user support or authentication
- Configuration file for custom settings
- Performance optimization for large lists (acceptable to optimize if trivial)
- Colored output or fancy formatting
- Aliases beyond `rm` for delete command

---

## Convergence Signal: **APPROVED FOR IMPLEMENTATION**

### Blocker Resolution

✅ **Critical Blocker 1 - Delete Semantics:** Hard delete chosen. Todos are removed from JSON file, not marked as complete. Data model simplified: no `completed` field. All sections updated to reflect this clear, unambiguous behavior.

✅ **Critical Blocker 2 - CLI Parsing:** Clap is now **required**, not optional. This ensures proper handling of quoted arguments and spaces in task descriptions. Specification explicitly forbids `std::env::args()`.

✅ **Secondary Issue 1 - File Corruption:** Robustness section added. Corruption detection strategy clear: user can delete file to reset.

✅ **Secondary Issue 2 - ID Persistence:** Clarified: IDs never reuse. Next ID = max existing ID + 1.

✅ **Secondary Issue 3 - Environment Variable:** `TODO_FILE` specified as override for default `~/.todos.json`.

✅ **Secondary Issue 4 - Whitespace-Only Descriptions:** Explicitly rejected alongside empty descriptions.

✅ **Atomic Writes:** Documented as preferred; direct overwrites acceptable if needed.

### Requirements Coverage
- ✓ Rust CLI application with add/list/delete commands
- ✓ JSON file storage with clear data model
- ✓ User-friendly error handling
- ✓ Persistent storage between sessions
- ✓ Unambiguous specification for implementers

### Implementation Readiness
**All critical ambiguities resolved.** Implementers have clear, actionable requirements. No guessing required. Ready to proceed to implementation phase.

---

## Version History

| Version | Date       | Author      | Change |
|---------|------------|-------------|--------|
| 1.0     | 2026-05-07 | spec-writer | Initial draft (rejected on review) |
| 2.0     | 2026-05-07 | spec-writer | Revised: hard delete semantics, required clap, robustness section, ID/env var clarifications |
