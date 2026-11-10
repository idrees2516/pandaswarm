//! Git operations for the swarm: per-session repos, per-agent branches and
//! worktrees, patch export (cezar-style PR flow) — via the `git` CLI.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .with_context(|| format!("git {}", args.join(" ")))?;
    if !out.status.success() {
        anyhow::bail!(
            "git {} failed: {}{}",
            args.join(" "),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// Ensure `dir` is a git repo (init if needed) with a configured identity.
pub fn ensure_repo(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    if !dir.join(".git").exists() {
        git(dir, &["init", "-q", "-b", "main"])?;
        git(dir, &["config", "user.email", "swarm@pandaswarm.local"])?;
        git(dir, &["config", "user.name", "PandaSwarm"])?;
        // allow empty commits later
        git(dir, &["commit", "-q", "--allow-empty", "-m", "pandaswarm: session workspace"])?;
    }
    Ok(())
}

/// Commit everything under `dir` (if anything changed).
pub fn commit_all(dir: &Path, message: &str) -> Result<String> {
    ensure_repo(dir)?;
    let _ = git(dir, &["add", "-A"]);
    let status = git(dir, &["status", "--porcelain"])?;
    if status.trim().is_empty() {
        return Ok("nothing to commit".into());
    }
    git(dir, &["commit", "-q", "-m", message])?;
    Ok(git(dir, &["rev-parse", "--short", "HEAD"])?.trim().to_string())
}

/// Create a branch for an agent's workspace.
pub fn agent_branch(dir: &Path, agent_name: &str) -> Result<String> {
    ensure_repo(dir)?;
    let branch = format!("pandaswarm/{agent_name}");
    let branches = git(dir, &["branch", "--format=%(refname:short)"])?;
    if branches.lines().any(|b| b.trim() == branch) {
        return Ok(branch);
    }
    git(dir, &["branch", &branch])?;
    Ok(branch)
}

/// Materialise an agent workspace as a git worktree of the session repo.
pub fn worktree(session_repo: &Path, agent_name: &str, target: &Path) -> Result<()> {
    ensure_repo(session_repo)?;
    let branch = agent_branch(session_repo, agent_name)?;
    if target.exists() {
        return Ok(());
    }
    git(
        session_repo,
        &["worktree", "add", "-q", &target.display().to_string(), &branch],
    )?;
    Ok(())
}

/// Export the diff of `head` relative to `base` as a unified patch file.
pub fn export_patch(dir: &Path, base: &str, head: &str, out_path: &Path) -> Result<()> {
    let patch = git(dir, &["diff", base, head])?;
    std::fs::write(out_path, patch)?;
    Ok(())
}

/// Recent commits, newest first.
pub fn log(dir: &Path, limit: usize) -> Result<Vec<String>> {
    ensure_repo(dir)?;
    Ok(git(dir, &["log", &format!("-{limit}"), "--oneline", "--no-decorate"])?
        .lines()
        .map(String::from)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_init_commit_and_branch() {
        let dir = std::env::temp_dir().join(format!("psgit-{}", pandaswarm_core::new_id("t")));
        std::fs::create_dir_all(&dir).unwrap();
        ensure_repo(&dir).unwrap();
        std::fs::write(dir.join("hello.txt"), "hi").unwrap();
        let sha = commit_all(&dir, "test: add hello").unwrap();
        assert!(!sha.is_empty());
        let branch = agent_branch(&dir, "coder-1").unwrap();
        assert_eq!(branch, "pandaswarm/coder-1");
        let commits = log(&dir, 5).unwrap();
        assert!(commits.len() >= 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
