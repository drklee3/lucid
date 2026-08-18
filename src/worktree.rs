//! Per-issue git worktree isolation — every dispatch gets its own branch and
//! checkout instead of running in the shared `daemon.workdir`, so a harness's
//! `git add -A && git commit` can never scoop up an unrelated in-progress edit or
//! another issue's uncommitted work. See docs/wiki/architecture/worker-completion.md.

use anyhow::Context;
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub struct WorktreeHandle {
    pub path: PathBuf,
    pub branch: String,
}

/// `lucid/<issue-id>`, with anything that isn't ASCII alphanumeric/`-`/`_`/`.`
/// collapsed to `-` — tracker issue ids are usually already branch-safe (`ENG-142`,
/// a UUID), but nothing enforces that for a hand-filed tracker item.
#[must_use]
pub fn branch_name(issue_id: &str) -> String {
    let slug: String = issue_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    format!("lucid/{slug}")
}

async fn run_git(repo_root: &Path, args: &[&str]) -> anyhow::Result<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_root)
        .output()
        .await
        .with_context(|| {
            format!(
                "running `git {}` in {}",
                args.join(" "),
                repo_root.display()
            )
        })?;
    if !output.status.success() {
        anyhow::bail!(
            "git {} failed in {}: {}",
            args.join(" "),
            repo_root.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

/// Creates a fresh worktree checked out on a new branch off `base_branch`'s
/// current tip — `worktree_root/<branch-slug>`. `worktree_root` is deliberately
/// outside `repo_root` (defaults to a temp dir): `git worktree add` refuses a path
/// that's already tracked, and keeping worktrees out of the main tree also means
/// they never show up in `repo_root`'s own `git status`.
///
/// # Errors
/// Returns an error if `repo_root` isn't a git repository, `base_branch` doesn't
/// exist, or the worktree add itself fails (e.g. the branch name is already
/// checked out elsewhere).
pub async fn create(
    repo_root: &Path,
    worktree_root: &Path,
    base_branch: &str,
    issue_id: &str,
) -> anyhow::Result<WorktreeHandle> {
    let branch = branch_name(issue_id);
    let path = worktree_root.join(branch.replace('/', "-"));

    std::fs::create_dir_all(worktree_root)
        .map_err(|e| anyhow::anyhow!("creating worktree root {}: {e}", worktree_root.display()))?;

    // A leftover directory from a prior crashed run would make `worktree add`
    // fail outright rather than silently reuse stale state.
    if path.exists() {
        let _ = std::fs::remove_dir_all(&path);
    }

    // A retry of a `Failed`/`TimedOut` run (`daemon::dispatch_approved_issues`
    // picks the issue back up on the next tick) reuses this same branch name —
    // `remove` deliberately never deletes it (see its doc comment), so without
    // this, `worktree add -b` would fail outright with "branch already exists"
    // on every retry. Safe to force-delete: the only thing that ever creates
    // `lucid/*` branches is this function, and a prior attempt's commits (if any)
    // either already made it into a still-open PR (safe on the remote) or were
    // never pushed at all (not worth preserving over letting the retry proceed).
    // `worktree prune` first clears any stale administrative record for the
    // directory just removed above, which `branch -D` would otherwise refuse to
    // touch while a (now-deleted) worktree still appears to reference it.
    let _ = run_git(repo_root, &["worktree", "prune"]).await;
    let _ = run_git(repo_root, &["branch", "-D", &branch]).await;

    run_git(
        repo_root,
        &[
            "worktree",
            "add",
            "-b",
            &branch,
            path.to_str().ok_or_else(|| {
                anyhow::anyhow!("worktree path {} isn't valid UTF-8", path.display())
            })?,
            base_branch,
        ],
    )
    .await?;

    Ok(WorktreeHandle { path, branch })
}

/// Removes the worktree checkout (`--force` so uncommitted stray files, e.g. a
/// harness leaving build artifacts, don't block cleanup) and prunes its
/// registration. Does **not** delete `handle.branch` — a merged branch is deleted
/// by `pr::merge`'s `--delete-branch`; an unmerged one (still under review, or the
/// dispatch failed) needs to survive worktree cleanup so its history isn't lost.
///
/// # Errors
/// Returns an error if `git worktree remove` itself fails — the caller should log
/// and continue rather than treat this as fatal, since the dispatch's actual
/// outcome has already been decided by this point.
pub async fn remove(repo_root: &Path, handle: &WorktreeHandle) -> anyhow::Result<()> {
    let path = handle.path.to_str().ok_or_else(|| {
        anyhow::anyhow!("worktree path {} isn't valid UTF-8", handle.path.display())
    })?;
    run_git(repo_root, &["worktree", "remove", "--force", path]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo(dir: &Path) {
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap()
        };
        run(&["init", "-q", "-b", "main"]);
        run(&["config", "user.email", "test@example.com"]);
        run(&["config", "user.name", "test"]);
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "init"]);
    }

    #[test]
    fn branch_name_slugifies_unsafe_chars() {
        assert_eq!(branch_name("ENG-142"), "lucid/ENG-142");
        assert_eq!(
            branch_name("weird id/with spaces"),
            "lucid/weird-id-with-spaces"
        );
    }

    #[tokio::test]
    async fn create_then_remove_round_trips() {
        let repo_root =
            std::env::temp_dir().join(format!("lucid-wt-repo-{}", uuid::Uuid::new_v4()));
        let worktree_root =
            std::env::temp_dir().join(format!("lucid-wt-root-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&repo_root).unwrap();
        init_repo(&repo_root);

        let handle = create(&repo_root, &worktree_root, "main", "ENG-1")
            .await
            .unwrap();
        assert_eq!(handle.branch, "lucid/ENG-1");
        assert!(handle.path.join("a.txt").exists());

        remove(&repo_root, &handle).await.unwrap();
        assert!(!handle.path.exists());

        let _ = std::fs::remove_dir_all(&repo_root);
        let _ = std::fs::remove_dir_all(&worktree_root);
    }

    /// A `Failed`/`TimedOut` run leaves its `lucid/<issue-id>` branch behind on
    /// purpose (`remove`'s doc comment); `daemon::dispatch_approved_issues` then
    /// retries the same issue on the next tick, calling `create` again with the
    /// identical issue id. That retry must succeed, not fail with "branch already
    /// exists".
    #[tokio::test]
    async fn create_after_a_failed_attempt_reuses_the_branch_name() {
        let repo_root =
            std::env::temp_dir().join(format!("lucid-wt-retry-repo-{}", uuid::Uuid::new_v4()));
        let worktree_root =
            std::env::temp_dir().join(format!("lucid-wt-retry-root-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&repo_root).unwrap();
        init_repo(&repo_root);

        let first = create(&repo_root, &worktree_root, "main", "ENG-1")
            .await
            .unwrap();
        remove(&repo_root, &first).await.unwrap();

        let second = create(&repo_root, &worktree_root, "main", "ENG-1")
            .await
            .unwrap();
        assert_eq!(second.branch, "lucid/ENG-1");
        assert!(second.path.join("a.txt").exists());
        remove(&repo_root, &second).await.unwrap();

        let _ = std::fs::remove_dir_all(&repo_root);
        let _ = std::fs::remove_dir_all(&worktree_root);
    }
}
