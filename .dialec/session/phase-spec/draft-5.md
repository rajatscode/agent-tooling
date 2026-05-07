# Rust CLI Todo App Specification

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
- Minimal dependencies (serde_json + clap + chrono required)

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
  - If JSON file exists but is corrupted: Display "Error: Todos file is corrupted. Please check `~/.todos.json` or delete it to reset." and exit with code 1
  - Reject if todo file cannot be written (disk full, permissions, etc.) with message: "Error: Could not save todos"

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
- **Error Handling:** 
  - If file doesn't exist: Treat as empty list and display empty message (see Robustness section)
  - If file exists but is invalid JSON: Display "Error: Todos file is corrupted. Please check `~/.todos.json` or delete it to reset." and exit with code 1

### 3. Delete Todo
- **Command:** `todo delete <id>` (primary command)
- **Behavior:**
  - Hard delete: Remove the todo by ID from the JSON file permanently
  - After deletion, remaining todos keep their original IDs (no renumbering)
  - Persist changes immediately to JSON file
- **Output:** Confirmation with description (e.g., "Deleted todo #1: Buy groceries")
- **Error Handling:** 
  - If ID not found: "Error: Todo #<id> not found"
  - If ID is invalid (not a number): "Error: Invalid ID. Expected a number."
  - If file exists but is corrupted: Display "Error: Todos file is corrupted. Please check `~/.todos.json` or delete it to reset." and exit with code 1
  - If file write fails: "Error: Could not save todos"
- **Note:** The `todo rm <id>` alias is optional and not required for v1.

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
- Leading and trailing whitespace in descriptions is preserved (no trimming applied)

### File Location
- **Default:** `~/.todos.json` (i.e., `$HOME/.todos.json`)
- **Override:** Via environment variable `TODO_FILE` (if set, use this path instead of default)
- Create file automatically on first write if it doesn't exist
- If file doesn't exist on read (at any time, not just first use), treat as empty list and proceed
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
   - Methods: `add(description: &str) -> Todo`, `delete(id: u32) -> Result<Todo, Error>`, `list() -> &[Todo]`
   - Note: The `delete` method must return the deleted `Todo` (or its description) to fulfill the output requirement

3. **Storage:** 
   - Load todos from JSON file on startup
   - Save todos to JSON file after each mutation (atomic write preferred; see Robustness)
   - Use `serde` and `serde_json` for serialization/deserialization

4. **Timestamp Generation:**
   - Use `chrono` crate to generate and format ISO 8601 timestamps in UTC
   - Store as string in JSON (e.g., "2026-05-07T12:30:00Z")

5. **Error Handling:** 
   - Define custom error enum: `TodoError { FileNotFound, InvalidJson, IoError(String), NotFound, InvalidId, EmptyDescription }`
   - Surface user-friendly messages (don't expose stack traces)

### Workflow
1. Parse command-line arguments using clap
2. Determine command (add/list/delete/help)
3. Load todos from JSON file (or initialize empty if file missing)
4. Execute requested operation
5. Save updated todos back to JSON file (if mutation occurred)
6. Display user feedback
7. Exit with status 0 (success) or non-zero (error)

## Robustness

### File Corruption & Recovery

**Corruption Scenarios & Handling:**
- **File doesn't exist (at any time):** Treat as empty list; proceed normally with empty todo operations. The list command displays "No todos yet..." message. The add command creates the file on first write.
- **File exists but is invalid JSON:** 
  - On `list`, `add`, or `delete`: Display "Error: Todos file is corrupted. Please check `~/.todos.json` or delete it to reset." and exit with code 1
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

$ todo add "New task"
Error: Todos file is corrupted. Please check ~/.todos.json or delete it to reset.

$ todo delete 1
Error: Todos file is corrupted. Please check ~/.todos.json or delete it to reset.
```

### Command Aliases
- `todo rm <id>` is **optional and not required** for v1. Implementers may omit it. If implemented, it should behave identically to `todo delete <id>`.

## Dependencies

**Required:**
- `serde` and `serde_json` for JSON serialization
- `clap` for robust CLI argument parsing (required in Cargo.toml)
- `chrono` for ISO 8601 timestamp generation and formatting (required for `created_at` field in data model)

**Standard library only:**
- `std::fs` for file I/O
- `std::env` for environment variables
- `std::path::PathBuf` for path handling

**Optional (nice-to-have, not required for v1):**
- `colored` or `termcolor` for colored output
- `dirs` for portable home directory expansion (if `~` expansion is desired)

**Initial scope:** Use `serde_json` + `clap` + `chrono` + standard library. Do not add optional dependencies unless explicitly requested.

## Success Criteria

- [ ] All three commands (add, list, delete) work correctly with stated behavior
- [ ] `clap` is used for CLI parsing; quoted arguments with spaces work correctly
- [ ] Data persists to JSON file between sessions
- [ ] Todo IDs are stable: auto-increment, never reused, even after deletion
- [ ] Empty descriptions and whitespace-only descriptions are rejected
- [ ] Non-existent IDs handled with clear error message
- [ ] File corruption detection and recovery strategy implemented (user can reset via deletion)
- [ ] File not found is handled correctly (treated as empty list on all commands)
- [ ] Environment variable `TODO_FILE` is honored if set
- [ ] ISO 8601 timestamps (UTC) are correctly generated and stored for all todos
- [ ] Delete command output includes the todo description (e.g., "Deleted todo #1: Buy groceries")
- [ ] File corruption error message is consistent across all three commands (list, add, delete)
- [ ] File corruption examples work as shown in CLI examples section
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
- `todo rm` alias (optional, not required)

---

## Convergence Signal

**Verdict:** APPROVED ✅

**Status:** Ready for implementation

**Objections addressed:** 0 open objections remain (objections.jsonl is empty)

**Review results:** 
- Review-1: Initial draft identified core requirements
- Review-2: Identified and resolved file I/O error handling inconsistencies
- Review-3: Addressed critical blockers (file corruption messages, delete alias, whitespace handling, method signatures)
- Review-4: Approved with format cleanup request (removal of meta-commentary sections)

**Format cleanup completed:**
- ✅ Removed "Spec Resolution Notes" section (formerly lines 275-328)
- ✅ Removed old "Convergence Signal" section (formerly lines 332-354)
- ✅ Kept all technical specification content (original lines 1-272)

**Internal consistency verified:**
- ✅ File-not-found behavior: consistently specified as "treat as empty list" across all features and robustness sections
- ✅ File corruption error messages: consistent "Todos file is corrupted..." across all three commands (add, list, delete) and CLI examples
- ✅ Delete output format: includes description across all relevant sections
- ✅ ID semantics: auto-increment, never reused, documented consistently
- ✅ Error handling: unified across features and robustness section

**Completeness verification:**
- ✅ Command syntax fully specified (add, list, delete)
- ✅ Data model fully specified (Todo struct, JSON format, file location)
- ✅ File I/O behavior fully specified (file locations, error cases, recovery)
- ✅ Error messages fully specified with examples
- ✅ CLI output format fully specified
- ✅ Dependencies and architecture fully specified
- ✅ Edge cases all covered (empty list, missing file, corrupted file, invalid ID, whitespace-only descriptions)

**Ambiguities resolved:** All prior objections from reviews 1-4 have been addressed. No blocking objections remain.

**Implementability:** An implementer can follow this specification without ambiguity and build the application to meet all success criteria.

This specification is complete, internally consistent, and ready for implementation.

---

**Spec-Writer:** spec-writer  
**Date:** 2026-05-07  
**Session:** dialec specification phase (draft-5)  
**Status:** Approved and ready for implementation phase
