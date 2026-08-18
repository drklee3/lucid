//! Pushes a worktree's branch and opens/merges its PR via the `gh` CLI — the
//! actual "merge" authority described in
//! docs/wiki/architecture/worker-completion.md: lucid never resolves a conflict
//! itself, it only asks `gh` to merge and reports back whatever `gh` says.

use std::path::Path;
use tokio::process::Command;

pub struct PullRequest {
    pub url: String,
    pub branch: String,
}

/// `git push -u origin <branch>` from inside the worktree.
///
/// # Errors
/// Returns an error if the push itself fails (no remote, no credentials, rejected
/// push) — this is always run against a freshly-created branch, so a rejection
/// means something is wrong with the remote/auth, not a real conflict to resolve.
pub async fn push_branch(worktree_path: &Path, branch: &str) -> anyhow::Result<()> {
    let output = Command::new("git")
        .args(["push", "-u", "origin", branch])
        .current_dir(worktree_path)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("running `git push` in {}: {e}", worktree_path.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "git push of {branch} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Opens a PR for an already-pushed branch via `gh pr create`, returning the PR's
/// URL (what `gh` prints to stdout on success).
///
/// # Errors
/// Returns an error if `gh` isn't installed/authenticated, or the create call
/// itself fails (e.g. a PR for this branch already exists).
pub async fn create(
    repo_root: &Path,
    branch: &str,
    base_branch: &str,
    title: &str,
    body: &str,
) -> anyhow::Result<PullRequest> {
    let output = Command::new("gh")
        .args([
            "pr",
            "create",
            "--head",
            branch,
            "--base",
            base_branch,
            "--title",
            title,
            "--body",
            body,
        ])
        .current_dir(repo_root)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("running `gh pr create` in {}: {e}", repo_root.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "gh pr create for {branch} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(PullRequest {
        url,
        branch: branch.to_string(),
    })
}

/// Merges a PR by branch name (`gh pr merge --squash --delete-branch`) — the only
/// merge authority lucid has. A failure here (unmet required checks, merge
/// conflict, branch protection) is reported as `Err(<gh's own message>)` for the
/// caller to route to `NeedsReview`, never retried or resolved automatically: see
/// docs/wiki/architecture/worker-completion.md § who merges.
///
/// # Errors
/// Returns an error (containing `gh`'s own stderr) whenever `gh pr merge` itself
/// exits non-zero.
pub async fn merge(repo_root: &Path, branch: &str) -> anyhow::Result<()> {
    let output = Command::new("gh")
        .args(["pr", "merge", branch, "--squash", "--delete-branch"])
        .current_dir(repo_root)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("running `gh pr merge` in {}: {e}", repo_root.display()))?;
    if !output.status.success() {
        anyhow::bail!(
            "gh pr merge of {branch} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}
