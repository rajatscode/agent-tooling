# Rust CLI Todo App Specification

## Executive Summary

Build a lightweight command-line todo application in Rust that allows users to manage a personal task list stored persistently in JSON format. The app provides three core operations: add new todos, list existing todos, and delete completed or unwanted tasks.

## Goals & Constraints

**Goals:**
- Create a simple, single-file JSON-based persistence layer
- Implement three essential todo management operations
- Provide clear, user-friendly CLI feedback
- Ensure reliable data persistence and recovery

**Constraints:**
- No external database; JSON file only
- No GUI or advanced features
- Single user (no authentication/multi-user support needed)
- Minimal dependencies preferred

## Core Features

### 1. Add Todo
- **Command:** `todo add <task-description>`
- **Behavior:**
  - Accept a task description as a CLI argument
  - Assign a unique ID (auto-incrementing integer)
  - Record creation timestamp
  - Mark as incomplete by default
  - Persist immediately to JSON file
- **Output:** Confirmation with assigned ID (e.g., "Added todo #1: Buy milk")
- **Error Handling:** Reject empty descriptions with helpful message

### 2. List Todos
- **Command:** `todo list` or `todo`
- **Behavior:**
  - Display all todos in a readable table/list format
  - Show: ID, status (checkbox or text), description
  - Order by creation time (oldest first) or by ID
  - Indicate completion status visually
- **Output:** Formatted list (e.g., `[ ] 1. Buy milk` / `[x] 2. Review PR`)
- **Edge Case:** Display friendly message if list is empty

### 3. Delete Todo
- **Command:** `todo delete <id>` or `todo done <id>`
- **Behavior:**
  - Remove todo by ID or mark as complete (implementation choice)
  - Handle invalid/non-existent IDs gracefully
  - Persist changes immediately
- **Output:** Confirmation (e.g., "Deleted todo #1" or "Marked #1 as done")
- **Error Handling:** Clear error for missing or invalid IDs

## Data Model

### Todo Structure
```json
{
  "todos": [
    {
      "id": 1,
      "description": "Buy milk",
      "completed": false,
      "created_at": "2026-05-07T12:30:00Z"
    }
  ]
}
```

### File Location
- Default: `~/.todos.json` (or configurable via environment variable)
- Create file automatically on first use
- Ensure proper permissions (readable/writable by user only)

## Architecture & Implementation

### Project Structure
```
src/
  main.rs          # CLI argument parsing and command dispatch
  todo.rs          # Todo struct and core logic
  storage.rs       # JSON file I/O operations
  cli.rs           # User-facing message formatting
```

### Key Components
1. **CLI Parser:** Use `std::env::args()` or lightweight crate (e.g., `clap` optional)
2. **Todo Manager:** In-memory struct managing current todos
3. **Storage:** Load/save todos from/to JSON file (use `serde_json`)
4. **Error Handling:** Custom error types for file I/O, invalid IDs, etc.

### Workflow
1. Parse command-line arguments
2. Load todos from JSON file (or create if missing)
3. Execute requested operation (add/list/delete)
4. Persist updated todos back to JSON
5. Display user-friendly feedback

## CLI Behavior & Examples

### Usage Examples
```bash
$ todo add "Buy groceries"
Added todo #1: Buy groceries

$ todo list
[ ] 1. Buy groceries
[ ] 2. Call dentist

$ todo delete 1
Deleted todo #1: Buy groceries

$ todo
[ ] 2. Call dentist
```

### Error Cases
```bash
$ todo add ""
Error: Task description cannot be empty

$ todo delete 999
Error: Todo #999 not found

$ todo list
(empty list)
No todos yet. Add one with: todo add "<task>"
```

## Dependencies

**Core Dependencies:**
- `serde` and `serde_json` for JSON serialization (required)
- Standard library only for I/O and basic types

**Optional (for improved UX):**
- `clap` for robust CLI argument parsing
- `colored` or `termcolor` for colored output
- `anyhow` for simplified error handling

**Initial scope:** Use standard library + `serde_json` only. Add optional dependencies if time permits.

## Success Criteria

- [ ] All three commands (add, list, delete) work correctly
- [ ] Data persists to JSON file between sessions
- [ ] Todo IDs are stable and auto-increment
- [ ] Clear, helpful error messages for invalid input
- [ ] Empty descriptions and non-existent IDs handled gracefully
- [ ] Code compiles without warnings
- [ ] Basic manual testing confirms expected behavior

## Out of Scope (Phase 1)

- Edit/update existing todos
- Filtering or searching
- Priority levels or categories
- Due dates or reminders
- Multi-user support
- Configuration file for custom settings
- Performance optimization for large todo lists

---

## Convergence Signal

**Ready to proceed to implementation?**

This spec covers all requirements in the user goal:
✓ Rust CLI application  
✓ JSON file storage  
✓ Add command  
✓ List command  
✓ Delete command  

**Open questions resolved:**
- Data model is simple and clear
- File location established (home directory)
- Error handling strategy defined
- Implementation approach straightforward

**No blocking objections identified.** Proceed to implementation phase.
