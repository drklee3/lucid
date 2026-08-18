//! Pushes a worktree's branch and opens/merges its PR via the `gh` CLI — the
//! actual "merge" authority described in
//! docs/wiki/architecture/worker-completion.md: lucid never resolves a conflict
//! itself, it only asks `gh` to merge and reports back whatever `gh` says.

use serde::Deserialize;
use std::path::Path;
use tokio::process::Command;

pub struct PullRequest {
    pub url: String,
    pub branch: String,
}

/// A PR's merge status, as reported by `gh pr view --json state`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrStatus {
    Open,
    Merged,
    Closed,
}

#[derive(Deserialize)]
struct PrView {
    state: String,
}

/// Looks up an existing PR's status by branch name — the read half of the
/// merge-status reconciliation loop (see docs/wiki/architecture/worker-completion.md):
/// a human merging/closing a `NeedsReview` PR by hand on GitHub is the "decision"
/// this reads back, never one lucid makes itself.
///
/// # Errors
/// Returns an error if `gh` itself fails to run or exits non-zero for a reason
/// other than "no PR found for this branch" — that case returns `Ok(None)`.
pub async fn status(repo_root: &Path, branch: &str) -> anyhow::Result<Option<PrStatus>> {
    let output = Command::new("gh")
        .args(["pr", "view", branch, "--json", "state"])
        .current_dir(repo_root)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("running `gh pr view` in {}: {e}", repo_root.display()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("no pull requests found") {
            return Ok(None);
        }
        anyhow::bail!("gh pr view of {branch} failed: {stderr}");
    }

    let view: PrView = serde_json::from_slice(&output.stdout)
        .map_err(|e| anyhow::anyhow!("parsing `gh pr view` output for {branch}: {e}"))?;
    Ok(Some(classify_state(&view.state)))
}

/// Maps `gh`'s own `state` field (`"OPEN"` / `"MERGED"` / `"CLOSED"`) to `PrStatus`
/// — any other value is treated as `Open` rather than erroring, since it's read-only
/// status that only ever gates whether reconciliation touches the issue.
fn classify_state(state: &str) -> PrStatus {
    match state {
        "MERGED" => PrStatus::Merged,
        "CLOSED" => PrStatus::Closed,
        _ => PrStatus::Open,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_state_maps_gh_states() {
        assert_eq!(classify_state("MERGED"), PrStatus::Merged);
        assert_eq!(classify_state("CLOSED"), PrStatus::Closed);
        assert_eq!(classify_state("OPEN"), PrStatus::Open);
        assert_eq!(classify_state("SOMETHING_UNKNOWN"), PrStatus::Open);
    }
}
