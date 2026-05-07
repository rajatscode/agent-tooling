# Claude CLI Subprocess Hanging Issue

## Summary
The `claude` CLI binary hangs indefinitely when spawned as a subprocess with piped stdio, preventing dialec from working with claude as a harness.

## Root Cause
The issue is **in the claude CLI itself**, not in dialec's subprocess handling code.

### Symptoms
- Process spawns successfully and gets a valid PID
- No output is produced before hanging (stdout and stderr remain empty)
- Process never exits and must be killed by timeout
- Works fine when invoked directly in shell (completes in ~65ms)

### Reproduction
```bash
# This works (runs directly):
/usr/local/bin/claude -p "test" --output-format json < /dev/null
# Exits with output after ~65ms

# This hangs (runs via subprocess):
cat > /tmp/spawn_test.rs << 'EOF'
use std::process::{Command, Stdio};
use std::io::Read;
use std::thread;

fn main() {
    let mut child = Command::new("/usr/local/bin/claude")
        .args(&["-p", "test", "--output-format", "json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn");
    
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    
    let stdout_handle = thread::spawn(move || {
        let mut data = Vec::new();
        stdout.read_to_end(&mut data).ok();
        data
    });
    
    // ... polling code ...
    // Process never exits, hangs forever
}
EOF
rustc /tmp/spawn_test.rs && timeout 10 /tmp/spawn_test
# Hangs until timeout
```

### Tested Configurations
Confirmed hanging with:
- `-p "prompt"` vs stdin piping
- With/without `--bare` flag
- With/without `--permission-mode bypassPermissions`
- With/without `--append-system-prompt-file`
- Prompt sizes from simple to 3400+ characters
- Timeouts from 10ms to 180 seconds

Only the direct shell invocation works.

## Impact
Dialec cannot use the `claude` harness in subprocess mode because agents never complete. The infrastructure is ready (activity logging, status queries, coordinator API) but cannot be tested.

## Workarounds
1. **Use tmux pane mode** - `dialec run --pane` spawns interactive claude in a tmux pane instead of subprocess
2. **Use codex harness** - codex works fine as subprocess and produces output correctly
3. **Wait for claude CLI fix** - the hanging is a claude CLI bug, not dialec code

## Files Modified
- `dialec/src/transaction.rs`:
  - Removed `--bare` flag (causes auth issues, see commit e780fbc)
  - Added `stdin(Stdio::null())` explicitly
  - These are correct fixes but do not resolve the hanging issue

## Investigation Details

### What Was Tested
1. Direct invocation: ✓ Works
2. Shell pipeline: ✓ Works
3. Subprocess with command-line args: ✗ Hangs
4. Subprocess with stdin piping: ✗ Hangs
5. Codex harness subprocess: ✓ Works
6. Long timeouts (180s): ✗ Still hangs

### stdout/stderr Behavior
- When hung: both empty (0 bytes)
- When direct invocation: correct JSON output received
- stderr does NOT show the "no stdin data" warning when subprocess hangs

### Process Behavior
- Spawning succeeds (valid PID obtained)
- try_wait() loop never returns Some(status)
- kill() successfully terminates process
- No zombie process (handled correctly by OS)

## Conclusion
This is a bug in the claude CLI binary, not dialec. The subprocess wrapper code is correct. The issue occurs at the claude application level when running as a non-TTY subprocess.
