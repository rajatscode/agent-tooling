use crate::fsutil::acquire_lock;
use crate::model::WorkspaceSnapshot;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn snapshot(path: &Path) -> WorkspaceSnapshot {
    match snapshot_result(path) {
        Ok(snapshot) => snapshot,
        Err(error) => WorkspaceSnapshot {
            path: path.to_string_lossy().to_string(),
            git_root: None,
            head: None,
            branch: None,
            status: vec![],
            dirty: false,
            error: Some(error.to_string()),
        },
    }
}

fn snapshot_result(path: &Path) -> Result<WorkspaceSnapshot> {
    let git_root = git_output(path, ["rev-parse", "--show-toplevel"])?;
    let head = git_output(path, ["rev-parse", "HEAD"]).ok();
    let branch = git_output(path, ["rev-parse", "--abbrev-ref", "HEAD"]).ok();
    let status_raw = git_output(path, ["status", "--porcelain=v1", "-uall"])?;
    let status: Vec<String> = status_raw
        .lines()
        .map(|line| line.to_string())
        .filter(|line| !line.trim().is_empty())
        .collect();

    Ok(WorkspaceSnapshot {
        path: path.to_string_lossy().to_string(),
        git_root: Some(git_root),
        head,
        branch,
        dirty: !status.is_empty(),
        status,
        error: None,
    })
}

pub fn tracked_diff(path: &Path) -> String {
    let mut diff = String::new();
    if let Ok(staged) = git_output(path, ["diff", "--binary", "--cached", "HEAD"])
        && !staged.trim().is_empty()
    {
        diff.push_str("# staged diff\n");
        diff.push_str(&staged);
        if !diff.ends_with('\n') {
            diff.push('\n');
        }
    }
    if let Ok(unstaged) = git_output(path, ["diff", "--binary", "HEAD"])
        && !unstaged.trim().is_empty()
    {
        diff.push_str("# unstaged diff\n");
        diff.push_str(&unstaged);
        if !diff.ends_with('\n') {
            diff.push('\n');
        }
    }
    if let Ok(untracked) = untracked_file_diffs(path)
        && !untracked.trim().is_empty()
    {
        diff.push_str("# untracked file diffs\n");
        diff.push_str(&untracked);
        if !diff.ends_with('\n') {
            diff.push('\n');
        }
    }
    diff
}

fn untracked_file_diffs(path: &Path) -> Result<String> {
    let files = git_output(path, ["ls-files", "--others", "--exclude-standard"])?;
    let mut out = String::new();
    for rel in files.lines().filter(|line| !line.trim().is_empty()) {
        let full = path.join(rel);
        if !full.is_file() {
            continue;
        }
        let output = Command::new("git")
            .args(["diff", "--binary", "--no-index", "--", "/dev/null"])
            .arg(&full)
            .current_dir(path)
            .output()
            .with_context(|| format!("failed to diff untracked file {}", full.display()))?;
        if !output.stdout.is_empty() {
            out.push_str(&String::from_utf8_lossy(&output.stdout));
            if !out.ends_with('\n') {
                out.push('\n');
            }
        }
    }
    Ok(out)
}

pub fn create_worktree(root: &Path, name: &str, base: Option<&str>) -> Result<PathBuf> {
    let _lock = acquire_lock(root, "git-worktree")?;
    let path = root.join(".dialec").join("workspaces").join(name);
    if path.exists() {
        return Ok(path);
    }
    let branch = format!("dialec/{name}");
    let mut cmd = Command::new("git");
    cmd.args(["worktree", "add"])
        .arg(&path)
        .args(["-b", &branch]);
    if let Some(base) = base {
        cmd.arg(base);
    }
    let output = cmd
        .current_dir(root)
        .output()
        .with_context(|| format!("failed to create worktree {}", path.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "git worktree add failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(path)
}

pub fn remove_worktree(root: &Path, name: &str, delete_branch: bool) -> Result<()> {
    let _lock = acquire_lock(root, "git-worktree")?;
    let path = root.join(".dialec").join("workspaces").join(name);
    if path.exists() {
        let output = Command::new("git")
            .args(["worktree", "remove"])
            .arg(&path)
            .current_dir(root)
            .output()
            .with_context(|| format!("failed to remove worktree {}", path.display()))?;
        if !output.status.success() {
            anyhow::bail!(
                "git worktree remove failed: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }

    if delete_branch {
        delete_dialec_branch(root, name)?;
    }
    Ok(())
}

fn delete_dialec_branch(root: &Path, name: &str) -> Result<()> {
    let branch = format!("dialec/{name}");
    let exists = Command::new("git")
        .args([
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ])
        .current_dir(root)
        .status()
        .with_context(|| format!("failed to check branch {branch}"))?;
    if !exists.success() {
        return Ok(());
    }
    let output = Command::new("git")
        .args(["branch", "-D", &branch])
        .current_dir(root)
        .output()
        .with_context(|| format!("failed to delete branch {branch}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "git branch -d failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

pub fn commit_all(path: &Path, message: &str) -> Result<bool> {
    let status = git_output(path, ["status", "--porcelain=v1", "-uall"])?;
    if status.trim().is_empty() {
        return Ok(false);
    }

    let add = Command::new("git")
        .args(["add", "-A"])
        .current_dir(path)
        .output()
        .with_context(|| format!("failed to git add in {}", path.display()))?;
    if !add.status.success() {
        anyhow::bail!(
            "git add failed: {}{}",
            String::from_utf8_lossy(&add.stdout),
            String::from_utf8_lossy(&add.stderr)
        );
    }

    let commit = Command::new("git")
        .args(["commit", "-m", message])
        .current_dir(path)
        .output()
        .with_context(|| format!("failed to git commit in {}", path.display()))?;
    if !commit.status.success() {
        anyhow::bail!(
            "git commit failed: {}{}",
            String::from_utf8_lossy(&commit.stdout),
            String::from_utf8_lossy(&commit.stderr)
        );
    }
    Ok(true)
}

pub fn merge_branch(root: &Path, branch: &str, message: &str) -> Result<()> {
    let output = Command::new("git")
        .args(["merge", "--no-ff", branch, "-m", message])
        .current_dir(root)
        .output()
        .with_context(|| format!("failed to merge {branch}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "git merge failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

pub fn is_git_repo(path: &Path) -> bool {
    git_output(path, ["rev-parse", "--show-toplevel"]).is_ok()
}

pub fn list_worktrees(root: &Path) -> Result<String> {
    git_output(root, ["worktree", "list", "--porcelain"])
}

fn git_output<const N: usize>(path: &Path, args: [&str; N]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(path)
        .output()
        .with_context(|| format!("failed to run git in {}", path.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "git command failed in {}: {}{}",
            path.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}
