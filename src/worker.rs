//! Wires one dispatch attempt to its tracker item: run the harness, then post the
//! resulting trace link back as a proof-of-work artifact regardless of outcome —
//! see docs/wiki/architecture/trace-correlation.md and
//! docs/wiki/architecture/observability.md#proof-of-work-artifacts.
//!
//! Also owns what happens *after* a successful dispatch — see
//! `finalize_completion` and docs/wiki/architecture/worker-completion.md. This is
//! still just the per-issue wiring, not the reconciliation loop that decides *when*
//! to call it (stall-detect, retry policy, presence gating) — that's `daemon.rs`.

use crate::config::ObservabilityConfig;
use crate::harness::{self, DispatchError, DispatchRequest, HarnessProfile, TelemetryConfig};
use crate::pr::{self, PullRequest};
use crate::state::{ClaimState, ClaimedSubstate, WorkerPhase, WorkerRun};
use crate::tracker::{DecisionState, ReviewMode, TrackerAdapter, TrackerIssue};
use crate::worktree;
use chrono::Utc;
use std::path::Path;
use std::time::Duration;

/// Builds the dispatch prompt for a claimed issue: title as a heading, plus the
/// frontmatter+body handoff surface (see docs/wiki/architecture/agent-handoff.md)
/// when the tracker has one, plus a commit instruction — every dispatch runs in
/// its own worktree/branch now (see `worktree`), so there's no shared-directory
/// risk left in having the harness commit its own work. lucid pushes the branch
/// and opens the PR itself (see `pr`), so the harness is explicitly told not to.
/// Falls back to the bare title for issues created outside `create_proposal` (e.g.
/// hand-filed tracker items) — the Worker still dispatches, just with less to go on.
#[must_use]
pub fn dispatch_prompt(issue: &TrackerIssue) -> String {
    use std::fmt::Write;
    let mut prompt = match &issue.description {
        Some(description) => format!("# {}\n\n{description}", issue.title),
        None => issue.title.clone(),
    };
    let _ = write!(
        prompt,
        "\n\n---\n\nWhen you're done and confident any acceptance criteria above are \
         met, commit your changes yourself with `git commit` — one commit or several, \
         whatever's logically right for the change; include `{}` in at least one commit \
         message. Do not push, and do not open a pull request — lucid handles that itself. \
         If there's nothing worth committing, leave the working tree as it is.",
        issue.id
    );
    prompt
}

/// A Worker always dispatches under `auto` permission mode — full tool access
/// with classifier review, not the read-only surface a PM investigation gets (see
/// `pm::wake`'s `--allowedTools` list). See docs/wiki/architecture/harness-dispatch.md.
const WORKER_CLAUDE_ARGS: &[&str] = &["--permission-mode", "auto"];

/// Trace-query link for one dispatch, filtered to just that run's spans.
#[must_use]
pub fn trace_link(observability: &ObservabilityConfig, dispatch_id: &str) -> String {
    format!(
        "{}/projects/{}?filter=lucid.dispatch_id=='{dispatch_id}'",
        observability.trace_ui_base_url.trim_end_matches('/'),
        observability
            .trace_ui_project_id
            .as_deref()
            .unwrap_or("default"),
    )
}

/// `git rev-parse HEAD` in `workdir`, or `None` if it isn't a git repository (or
/// `git` isn't on `PATH`) — non-fatal either way, since a missing `git` binary
/// shouldn't crash the dispatch itself, only the commit-observation step.
async fn git_head(workdir: &Path) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(workdir)
        .output()
        .await
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

async fn git_dirty_file_count(workdir: &Path) -> Option<usize> {
    let output = tokio::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(workdir)
        .output()
        .await
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count()
    })
}

/// One-line `git log --oneline <before>..HEAD` entries — every commit the dispatch
/// made, not just whether `HEAD` moved. A harness is expected to make however many
/// commits are logically right for the change (see `dispatch_prompt`'s instruction),
/// not just one — comparing a single before/after SHA would silently drop all but
/// the last.
async fn commits_since(workdir: &Path, before: &str) -> Option<Vec<String>> {
    let output = tokio::process::Command::new("git")
        .args(["log", "--oneline", &format!("{before}..HEAD")])
        .current_dir(workdir)
        .output()
        .await
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(str::to_string)
            .collect()
    })
}

/// Describes what happened to the worktree since dispatch started, by listing
/// commits made — every dispatch runs in its own worktree and is told to commit
/// its own work (see `dispatch_prompt`), so lucid only *observes* the result here
/// rather than running `git add`/`git commit` itself; pushing and opening the PR
/// (if there's anything to push) happens separately in `dispatch_and_finalize`.
async fn describe_commit_result(workdir: &Path, head_before: Option<&str>) -> String {
    let Some(before) = head_before else {
        return "Commit status unknown (not a git repository, or `git` isn't available)."
            .to_string();
    };
    match commits_since(workdir, before).await {
        Some(commits) if !commits.is_empty() => {
            use std::fmt::Write;
            let mut out = format!("{} commit(s):", commits.len());
            for c in &commits {
                let _ = write!(out, "\n  - {c}");
            }
            out
        }
        Some(_) => match git_dirty_file_count(workdir).await {
            Some(0) => "No changes.".to_string(),
            Some(n) => format!("Left {n} file(s) uncommitted — nothing to open a PR for."),
            None => "Commit status unknown (not a git repository, or `git` isn't available)."
                .to_string(),
        },
        None => {
            "Commit status unknown (not a git repository, or `git` isn't available).".to_string()
        }
    }
}

/// Runs one dispatch attempt for `issue_id` and posts the trace link back to the
/// tracker whether it succeeded, failed, or was blocked on every profile — a failed
/// run's trace is exactly the one worth being able to find later.
///
/// Returns `Ok(WorkerRun)` even when the *dispatch itself* failed (that's a normal,
/// recorded outcome — see `run.phase`/`run.last_error`); this only returns `Err`
/// when posting the note back to the tracker fails, since that's an infrastructure
/// problem the caller needs to know about, not a run outcome.
///
/// # Errors
/// Returns an error only if `tracker.attach_note` itself fails.
#[allow(clippy::too_many_arguments)]
pub async fn run_dispatch(
    tracker: &dyn TrackerAdapter,
    issue_id: &str,
    prompt: &str,
    profiles: &[HarnessProfile],
    observability: &ObservabilityConfig,
    workdir: &Path,
    stall_timeout: Duration,
) -> anyhow::Result<WorkerRun> {
    use std::fmt::Write as _;

    let telemetry = TelemetryConfig {
        otlp_endpoint: observability.otlp_endpoint.clone(),
        log_prompts: observability.log_prompts,
    };

    let mut run = WorkerRun {
        issue_id: issue_id.to_string(),
        claim: ClaimState::Claimed(ClaimedSubstate::Running),
        phase: WorkerPhase::LaunchingAgentProcess,
        session_id: None,
        dispatch_id: None,
        retries: 0,
        last_event_at: Utc::now(),
        last_error: None,
    };

    let head_before = git_head(workdir).await;

    let outcome = harness::dispatch_with_fallback(DispatchRequest {
        profiles,
        prompt,
        ticket_id: issue_id,
        telemetry: &telemetry,
        workdir,
        timeout: stall_timeout,
        claude_extra_args: WORKER_CLAUDE_ARGS,
    })
    .await;

    let note = match &outcome {
        Ok(o) => {
            run.dispatch_id = Some(o.dispatch_id.clone());
            run.session_id = o.session_id.clone();
            // Prefer the harness's own `is_error` verdict (from the stream-json
            // result event) over exit status — a graceful classifier-driven skip in
            // auto mode can still exit 0 while `is_error` is what actually reports
            // task-level failure.
            let succeeded = match o.is_error {
                Some(is_error) => !is_error,
                None => o.status.is_some_and(|s| s.success()),
            };
            run.phase = if succeeded {
                WorkerPhase::Succeeded
            } else {
                WorkerPhase::Failed
            };
            let link = trace_link(observability, &o.dispatch_id);
            let status_desc = o
                .status
                .map_or_else(|| "unknown".to_string(), |s| s.to_string());
            let mut note = format!(
                "Dispatch `{}` via `{}` — status: {status_desc}\nTrace: {link}",
                o.dispatch_id, o.profile_name
            );
            let commit_status = describe_commit_result(workdir, head_before.as_deref()).await;
            let _ = write!(note, "\n{commit_status}");
            note
        }
        Err(e) => {
            // A timeout is a distinct, retriable state (`TimedOut`), not a plain
            // task failure — the reconciliation loop's retry policy treats these
            // differently (see docs/FEATURES.md § Reconciliation loop).
            run.phase = if matches!(
                e.downcast_ref::<DispatchError>(),
                Some(DispatchError::Timeout { .. })
            ) {
                WorkerPhase::TimedOut
            } else {
                WorkerPhase::Failed
            };
            run.last_error = Some(e.to_string());
            format!("Dispatch failed before completion: {e}")
        }
    };

    tracker.attach_note(issue_id, &note).await?;

    run.claim = ClaimState::Released;
    run.last_event_at = Utc::now();
    Ok(run)
}

/// Dispatches one already-`Approved` issue end to end: creates its worktree,
/// builds the prompt, runs the harness inside that worktree, pushes+opens a PR for
/// whatever it committed, finalizes the tracker's decision state per
/// `issue.review` (merging the PR when that decision is `Done`), and always tears
/// the worktree back down. Shared by `daemon::dispatch_approved_issues` (the
/// regular presence-gated tick) and `lucid task dispatch-now` (an on-demand
/// trigger of the *exact same* path, not a separate one) — see
/// docs/wiki/architecture/worker-completion.md.
///
/// # Errors
/// Returns an error if creating the worktree fails, or if `run_dispatch`/
/// `finalize_completion` does (tracker calls failing, not a normal dispatch
/// failure — see their own docs). Worktree teardown failures are logged to
/// stderr, not propagated — by the time cleanup runs, the dispatch's actual
/// outcome has already been decided and recorded.
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_and_finalize(
    tracker: &dyn TrackerAdapter,
    issue: &TrackerIssue,
    profiles: &[HarnessProfile],
    observability: &ObservabilityConfig,
    repo_root: &Path,
    worktree_root: &Path,
    base_branch: &str,
    stall_timeout: Duration,
    verify_cmd: Option<&str>,
) -> anyhow::Result<WorkerRun> {
    let wt = worktree::create(repo_root, worktree_root, base_branch, &issue.id).await?;

    let outcome = dispatch_and_review_in_worktree(
        tracker,
        issue,
        profiles,
        observability,
        repo_root,
        &wt,
        base_branch,
        stall_timeout,
        verify_cmd,
    )
    .await;

    // Deliberately removed *before* `finalize_completion` below: `pr::merge`'s
    // `gh pr merge --delete-branch` can't clean up a branch that's still checked
    // out in this worktree (`gh` surfaces that as a merge failure), so the merge
    // step has to run only after the checkout backing it is gone. Errors here are
    // logged, not propagated — by this point the dispatch's actual outcome (`run`,
    // below) has already been decided.
    if let Err(e) = worktree::remove(repo_root, &wt).await {
        eprintln!(
            "warning: failed to remove worktree {}: {e}",
            wt.path.display()
        );
    }

    let (run, verdict, pr_outcome) = outcome?;
    if let Some(verdict) = verdict {
        finalize_completion(tracker, issue, verdict, repo_root, &pr_outcome).await?;
    }
    Ok(run)
}

/// The inner half of `dispatch_and_finalize` that actually needs the worktree
/// alive: runs the harness, pushes+opens a PR for whatever it committed, and
/// decides *what the outcome should be* (`ReviewVerdict`) — but never merges. The
/// merge is deferred to `finalize_completion`, called only after the worktree
/// this ran in has been torn down (see `dispatch_and_finalize`).
#[allow(clippy::too_many_arguments)]
async fn dispatch_and_review_in_worktree(
    tracker: &dyn TrackerAdapter,
    issue: &TrackerIssue,
    profiles: &[HarnessProfile],
    observability: &ObservabilityConfig,
    repo_root: &Path,
    wt: &worktree::WorktreeHandle,
    base_branch: &str,
    stall_timeout: Duration,
    verify_cmd: Option<&str>,
) -> anyhow::Result<(WorkerRun, Option<ReviewVerdict>, PrOutcome)> {
    let prompt = dispatch_prompt(issue);
    let head_before = git_head(&wt.path).await;
    let run = run_dispatch(
        tracker,
        &issue.id,
        &prompt,
        profiles,
        observability,
        &wt.path,
        stall_timeout,
    )
    .await?;

    if run.phase != WorkerPhase::Succeeded {
        return Ok((run, None, PrOutcome::NoChanges));
    }

    let has_commits = match head_before.as_deref() {
        Some(before) => commits_since(&wt.path, before)
            .await
            .is_some_and(|c| !c.is_empty()),
        None => false,
    };
    let pr_outcome = if has_commits {
        match open_pr(repo_root, wt, base_branch, issue).await {
            Ok(pr) => {
                tracker
                    .attach_note(&issue.id, &format!("Opened PR: {}", pr.url))
                    .await?;
                PrOutcome::Created(pr)
            }
            Err(e) => {
                tracker
                    .attach_note(&issue.id, &format!("Failed to open a PR: {e}"))
                    .await?;
                PrOutcome::Failed
            }
        }
    } else {
        PrOutcome::NoChanges
    };

    let verdict = decide_review(
        tracker,
        issue,
        profiles,
        observability,
        &wt.path,
        stall_timeout,
        verify_cmd,
    )
    .await?;

    Ok((run, Some(verdict), pr_outcome))
}

/// Pushes `wt.branch` and opens its PR — the title is the issue title, the body
/// links back to the tracker item so a human clicking through from GitHub has
/// context without needing to already have the ticket open.
async fn open_pr(
    repo_root: &Path,
    wt: &worktree::WorktreeHandle,
    base_branch: &str,
    issue: &TrackerIssue,
) -> anyhow::Result<PullRequest> {
    pr::push_branch(&wt.path, &wt.branch).await?;
    let body = format!("Dispatched by lucid for tracker item `{}`.", issue.id);
    pr::create(repo_root, &wt.branch, base_branch, &issue.title, &body).await
}

/// A second, read-only dispatch that reviews a `ReviewMode::Agent` issue's pending
/// diff against its `acceptance_criteria` — reuses the same profile list and
/// dispatch mechanism as the Worker's own run, just with `pm::wake`'s restricted,
/// non-mutating `--allowedTools` surface (a reviewer has no business changing
/// files) instead of full `auto` access.
const REVIEWER_CLAUDE_ARGS: &[&str] = &[
    "--allowedTools",
    "Read,Grep,Glob,Bash(git diff *),Bash(git log *),Bash(git status *)",
];

/// Parses the reviewer's single-line verdict. `Some(true)`/`Some(false)` for a
/// clean `PASS`/`FAIL`; `None` when the reviewer didn't produce a parseable
/// verdict at all — treated as inconclusive (routes to `NeedsReview`, not a
/// silent pass) rather than guessing.
fn parse_verdict(text: &str) -> (Option<bool>, String) {
    for line in text.lines() {
        let line = line.trim();
        if line == "VERDICT: PASS" {
            return (Some(true), "Agent review: PASS".to_string());
        }
        if let Some(reason) = line.strip_prefix("VERDICT: FAIL:") {
            return (Some(false), format!("Agent review: FAIL —{reason}"));
        }
    }
    (
        None,
        format!("Agent review: couldn't parse a verdict from the reviewer's output:\n{text}"),
    )
}

/// Runs `cmd` (via `sh -c`) in `workdir` and reports whether it exited clean —
/// lucid checks the exit code itself rather than asking a review agent to
/// interpret whether tests passed, since an exit code is a fact and an LLM's
/// summary of one is a claim. See docs/wiki/architecture/worker-completion.md.
///
/// # Errors
/// Returns an error if the command can't even be spawned or times out — a
/// *failing* command (nonzero exit) is a normal `Ok(false)`, not an error.
async fn run_verify_cmd(workdir: &Path, cmd: &str, timeout: Duration) -> anyhow::Result<bool> {
    let output = tokio::time::timeout(
        timeout,
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(workdir)
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("verify command `{cmd}` timed out after {timeout:?}"))??;
    Ok(output.status.success())
}

async fn agent_review(
    issue: &TrackerIssue,
    profiles: &[HarnessProfile],
    observability: &ObservabilityConfig,
    workdir: &Path,
    stall_timeout: Duration,
) -> anyhow::Result<(Option<bool>, String)> {
    let telemetry = TelemetryConfig {
        otlp_endpoint: observability.otlp_endpoint.clone(),
        log_prompts: observability.log_prompts,
    };
    let prompt = format!(
        "You are reviewing a coding agent's uncommitted (or just-committed) work in \
         this repository against the following task.\n\n# {}\n\n{}\n\n\
         Run `git diff`/`git status`/`git log` as needed to see what actually \
         changed. Check it against any acceptance criteria above. Respond with \
         ONLY one line: `VERDICT: PASS` if the work satisfies the task, or \
         `VERDICT: FAIL: <one-sentence reason>` if it doesn't.",
        issue.title,
        issue.description.as_deref().unwrap_or("(no description)"),
    );

    let outcome = harness::dispatch_with_fallback(DispatchRequest {
        profiles,
        prompt: &prompt,
        ticket_id: &issue.id,
        telemetry: &telemetry,
        workdir,
        timeout: stall_timeout,
        claude_extra_args: REVIEWER_CLAUDE_ARGS,
    })
    .await?;

    Ok(parse_verdict(&outcome.result_text.unwrap_or_default()))
}

/// Resolves the command `ReviewMode::Agent` runs before its LLM diff review, in
/// priority order: an explicit per-task override (the exception — a docs-only
/// task, a monorepo task scoped to one package), then a repo-wide default
/// (`daemon.verify_cmd` — the common case, same command for every task, same shape
/// as a CI config). No auto-detection: guessing a command from repo conventions
/// (e.g. `cargo test`) would silently narrow "verified" to whatever that one guess
/// covers, while the repo's real CI might also lint, typecheck, or run several
/// suites — a partial check that looks like a real gate is worse than no gate.
/// `None` at both tiers leaves the review agent to infer its own command per task,
/// same as before `verify_cmd` existed at all.
fn resolve_verify_cmd(issue: &TrackerIssue, repo_default: Option<&str>) -> Option<String> {
    crate::tracker::frontmatter_field(issue.description.as_deref(), "verify_cmd")
        .or_else(|| repo_default.map(str::to_string))
}

/// What a `Succeeded` run's review step decided the tracker item's outcome should
/// be — computed by `decide_review` *before* any PR merge is attempted. Kept
/// distinct from the actual `DecisionState` transition (`finalize_completion`)
/// because the merge that `CloseAutomatically` implies has to wait until the
/// worktree it was reviewed in is gone (see `dispatch_and_finalize`).
#[derive(Debug)]
enum ReviewVerdict {
    CloseAutomatically,
    NeedsHuman,
}

/// What happened to the PR for a `Succeeded` run — distinguishes "nothing to
/// merge" from "something went wrong opening it," so a dispatch that committed
/// real work but failed to push/open a PR can never be silently treated as if it
/// had made no changes at all (see `mark_done`).
enum PrOutcome {
    NoChanges,
    Created(PullRequest),
    Failed,
}

/// Decides what a `Succeeded` dispatch means for the tracker item, per
/// `issue.review` — the review half of "how does a Worker finish a task" (see
/// docs/wiki/architecture/worker-completion.md). Doesn't touch `DecisionState`
/// itself; see `finalize_completion` for that.
///
/// # Errors
/// Returns an error if a tracker call fails, or (for `ReviewMode::Agent`) if the
/// review dispatch itself fails to even run — a review that ran but returned an
/// unparseable verdict is *not* an error, it resolves to `NeedsHuman` instead.
async fn decide_review(
    tracker: &dyn TrackerAdapter,
    issue: &TrackerIssue,
    profiles: &[HarnessProfile],
    observability: &ObservabilityConfig,
    workdir: &Path,
    stall_timeout: Duration,
    verify_cmd: Option<&str>,
) -> anyhow::Result<ReviewVerdict> {
    match issue.review {
        ReviewMode::Auto => Ok(ReviewVerdict::CloseAutomatically),
        ReviewMode::Human => Ok(ReviewVerdict::NeedsHuman),
        ReviewMode::Agent => {
            let verify_cmd = resolve_verify_cmd(issue, verify_cmd);
            if let Some(cmd) = &verify_cmd {
                match run_verify_cmd(workdir, cmd, stall_timeout).await {
                    Ok(true) => {} // falls through to the diff/acceptance-criteria review below
                    Ok(false) => {
                        tracker
                            .attach_note(
                                &issue.id,
                                &format!("Verify command `{cmd}` failed (nonzero exit)."),
                            )
                            .await?;
                        return Ok(ReviewVerdict::NeedsHuman);
                    }
                    Err(e) => {
                        tracker
                            .attach_note(
                                &issue.id,
                                &format!("Verify command `{cmd}` couldn't run: {e}"),
                            )
                            .await?;
                        return Ok(ReviewVerdict::NeedsHuman);
                    }
                }
            }

            let (verdict, note) =
                agent_review(issue, profiles, observability, workdir, stall_timeout).await?;
            tracker.attach_note(&issue.id, &note).await?;
            Ok(if verdict == Some(true) {
                ReviewVerdict::CloseAutomatically
            } else {
                ReviewVerdict::NeedsHuman
            })
        }
    }
}

/// Turns a `ReviewVerdict` into the tracker's actual `DecisionState` — split out
/// from `decide_review` so the merge `CloseAutomatically` can trigger
/// (`mark_done`) always runs after the worktree the PR came from has been torn
/// down (see `dispatch_and_finalize`'s call ordering).
///
/// # Errors
/// Returns an error only if a tracker call fails.
async fn finalize_completion(
    tracker: &dyn TrackerAdapter,
    issue: &TrackerIssue,
    verdict: ReviewVerdict,
    repo_root: &Path,
    pr_outcome: &PrOutcome,
) -> anyhow::Result<()> {
    match verdict {
        ReviewVerdict::NeedsHuman => {
            tracker
                .set_decision_state(&issue.id, DecisionState::NeedsReview)
                .await
        }
        ReviewVerdict::CloseAutomatically => mark_done(tracker, issue, repo_root, pr_outcome).await,
    }
}

/// The only place lucid ever merges a PR — reached solely from `ReviewVerdict::
/// CloseAutomatically`, i.e. exactly the two review outcomes (`ReviewMode::Auto`,
/// or a `PASS`ed `ReviewMode::Agent`) that already meant "close this out without
/// a human" before PRs existed. A merge failure (conflict, unmet branch
/// protection, required check still pending) is never retried or resolved
/// automatically: it routes to `NeedsReview` with `gh`'s own message attached,
/// leaving the PR open for a human — see docs/wiki/architecture/worker-completion.md
/// § who merges. `PrOutcome::Failed` (the dispatch committed real work, but lucid
/// couldn't push/open a PR for it) routes to `NeedsReview` the same way, rather
/// than being treated as if there had been nothing to merge — an unreviewed,
/// possibly-unpushed commit must never silently read back as `Done`.
async fn mark_done(
    tracker: &dyn TrackerAdapter,
    issue: &TrackerIssue,
    repo_root: &Path,
    pr_outcome: &PrOutcome,
) -> anyhow::Result<()> {
    match pr_outcome {
        PrOutcome::NoChanges => {
            tracker
                .set_decision_state(&issue.id, DecisionState::Done)
                .await
        }
        PrOutcome::Failed => {
            tracker
                .set_decision_state(&issue.id, DecisionState::NeedsReview)
                .await
        }
        PrOutcome::Created(pr) => {
            if let Err(e) = pr::merge(repo_root, &pr.branch).await {
                tracker
                    .attach_note(&issue.id, &format!("Could not merge PR {}: {e}", pr.url))
                    .await?;
                return tracker
                    .set_decision_state(&issue.id, DecisionState::NeedsReview)
                    .await;
            }
            tracker
                .set_decision_state(&issue.id, DecisionState::Done)
                .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{AuthMode, HarnessKind};
    use crate::tracker::{DecisionState, Proposal, ReviewMode, TrackerIssue};
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Records every note posted and every decision-state change, in place of a
    /// real tracker backend — none of Linear's actual GraphQL calls are
    /// implemented yet.
    #[derive(Default)]
    struct MockTracker {
        notes: Mutex<Vec<(String, String)>>,
        decisions: Mutex<Vec<(String, DecisionState)>>,
    }

    #[async_trait]
    impl TrackerAdapter for MockTracker {
        async fn create_proposal(&self, _proposal: &Proposal) -> anyhow::Result<String> {
            unimplemented!("not exercised by these tests")
        }
        async fn set_decision_state(
            &self,
            issue_id: &str,
            state: DecisionState,
        ) -> anyhow::Result<()> {
            self.decisions
                .lock()
                .unwrap()
                .push((issue_id.to_string(), state));
            Ok(())
        }
        async fn query_by_decision_state(
            &self,
            _state: DecisionState,
        ) -> anyhow::Result<Vec<TrackerIssue>> {
            unimplemented!("not exercised by these tests")
        }
        async fn query_similar(&self, _title: &str) -> anyhow::Result<Vec<TrackerIssue>> {
            unimplemented!("not exercised by these tests")
        }
        async fn attach_note(&self, issue_id: &str, body: &str) -> anyhow::Result<()> {
            self.notes
                .lock()
                .unwrap()
                .push((issue_id.to_string(), body.to_string()));
            Ok(())
        }
    }

    fn observability() -> ObservabilityConfig {
        ObservabilityConfig {
            otlp_endpoint: "http://localhost:4317".to_string(),
            log_prompts: false,
            trace_ui_base_url: "http://localhost:6006".to_string(),
            trace_ui_project_id: None,
        }
    }

    #[test]
    fn dispatch_prompt_includes_the_description_when_present() {
        let issue = TrackerIssue {
            id: "ENG-1".into(),
            title: "Fix presence detection".into(),
            description: Some("---\ntask_type: \"bug: fix\"\n---\n\nIdleHint never resets.".into()),
            decision_state: None,
            review: crate::tracker::ReviewMode::Auto,
        };
        let prompt = dispatch_prompt(&issue);
        assert!(prompt.starts_with("# Fix presence detection\n\n"));
        assert!(prompt.contains("IdleHint never resets."));
    }

    #[test]
    fn dispatch_prompt_appends_a_commit_instruction() {
        let issue = TrackerIssue {
            id: "ENG-4".into(),
            title: "Hand-filed issue".into(),
            description: None,
            decision_state: None,
            review: crate::tracker::ReviewMode::Auto,
        };
        let prompt = dispatch_prompt(&issue);
        assert!(prompt.starts_with("Hand-filed issue"));
        assert!(prompt.contains("git commit"));
        assert!(prompt.contains("ENG-4"));
        assert!(prompt.contains("Do not push"));
        assert!(prompt.contains("lucid handles that itself"));
    }

    #[test]
    fn trace_link_defaults_project_to_default() {
        let link = trace_link(&observability(), "abc-123");
        assert_eq!(
            link,
            "http://localhost:6006/projects/default?filter=lucid.dispatch_id=='abc-123'"
        );
    }

    #[tokio::test]
    async fn successful_dispatch_posts_a_trace_link_and_marks_succeeded() {
        let tracker = MockTracker::default();
        let profiles = [HarnessProfile {
            name: "fake-claude".into(),
            kind: HarnessKind::ClaudeCode,
            cmd: "true".into(),
            args: vec![],
            auth_mode: AuthMode::Subscription,
            priority: 1,
        }];

        let run = run_dispatch(
            &tracker,
            "ENG-1",
            "do the thing",
            &profiles,
            &observability(),
            &std::env::temp_dir(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert_eq!(run.phase, WorkerPhase::Succeeded);
        assert!(run.dispatch_id.is_some());
        assert_eq!(run.claim, ClaimState::Released);

        let notes = tracker.notes.lock().unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].0, "ENG-1");
        assert!(notes[0].1.contains("Trace: http://localhost:6006"));
        assert!(notes[0].1.contains(run.dispatch_id.as_ref().unwrap()));
    }

    #[tokio::test]
    async fn failing_dispatch_still_posts_a_note() {
        let tracker = MockTracker::default();
        let profiles = [HarnessProfile {
            name: "fake-claude".into(),
            kind: HarnessKind::ClaudeCode,
            cmd: "false".into(),
            args: vec![],
            auth_mode: AuthMode::Subscription,
            priority: 1,
        }];

        let run = run_dispatch(
            &tracker,
            "ENG-2",
            "do the thing",
            &profiles,
            &observability(),
            &std::env::temp_dir(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert_eq!(run.phase, WorkerPhase::Failed);
        let notes = tracker.notes.lock().unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].1.contains("Trace:"));
    }

    #[tokio::test]
    async fn no_profiles_still_posts_a_failure_note() {
        let tracker = MockTracker::default();
        let run = run_dispatch(
            &tracker,
            "ENG-3",
            "do the thing",
            &[],
            &observability(),
            &std::env::temp_dir(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert_eq!(run.phase, WorkerPhase::Failed);
        assert!(run.dispatch_id.is_none());
        let notes = tracker.notes.lock().unwrap();
        assert_eq!(notes.len(), 1);
        assert!(notes[0].1.contains("Dispatch failed before completion"));
    }

    #[tokio::test]
    async fn run_dispatch_reports_a_new_commit_in_the_note() {
        let workdir =
            std::env::temp_dir().join(format!("lucid-commit-mode-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workdir).unwrap();
        let run_git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&workdir)
                .output()
                .unwrap()
        };
        run_git(&["init", "-q"]);
        run_git(&["config", "user.email", "test@example.com"]);
        run_git(&["config", "user.name", "test"]);
        std::fs::write(workdir.join("a.txt"), "one").unwrap();
        run_git(&["add", "."]);
        run_git(&["commit", "-q", "-m", "init"]);

        let tracker = MockTracker::default();
        // Rather than spawning a real harness, the fake profile's own shell script
        // makes the commit a real harness dispatch would have made — the point under
        // test is `run_dispatch` *observing* it via `HEAD` before/after, not the
        // (already-covered) prompt instruction.
        let profiles = [HarnessProfile {
            name: "fake-claude".into(),
            kind: HarnessKind::ClaudeCode,
            cmd: "sh".into(),
            args: vec![
                "-c".into(),
                "echo two >> a.txt && git add -A && git commit -q -m done".into(),
            ],
            auth_mode: AuthMode::Subscription,
            priority: 1,
        }];

        let run = run_dispatch(
            &tracker,
            "ENG-5",
            "do the thing",
            &profiles,
            &observability(),
            &workdir,
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert_eq!(run.phase, WorkerPhase::Succeeded);
        let notes = tracker.notes.lock().unwrap();
        assert!(notes[0].1.contains("1 commit(s):"));
        assert!(notes[0].1.contains("done"));

        let _ = std::fs::remove_dir_all(&workdir);
    }

    #[tokio::test]
    async fn run_dispatch_reports_every_commit_not_just_the_last() {
        let workdir =
            std::env::temp_dir().join(format!("lucid-multi-commit-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workdir).unwrap();
        let run_git = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&workdir)
                .output()
                .unwrap()
        };
        run_git(&["init", "-q"]);
        run_git(&["config", "user.email", "test@example.com"]);
        run_git(&["config", "user.name", "test"]);
        std::fs::write(workdir.join("a.txt"), "one").unwrap();
        run_git(&["add", "."]);
        run_git(&["commit", "-q", "-m", "init"]);

        let tracker = MockTracker::default();
        let profiles = [HarnessProfile {
            name: "fake-claude".into(),
            kind: HarnessKind::ClaudeCode,
            cmd: "sh".into(),
            args: vec![
                "-c".into(),
                "echo two >> a.txt && git commit -q -am first && echo three >> a.txt && git commit -q -am second"
                    .into(),
            ],
            auth_mode: AuthMode::Subscription,
            priority: 1,
        }];

        let run = run_dispatch(
            &tracker,
            "ENG-9",
            "do the thing",
            &profiles,
            &observability(),
            &workdir,
            Duration::from_secs(5),
        )
        .await
        .unwrap();

        assert_eq!(run.phase, WorkerPhase::Succeeded);
        let notes = tracker.notes.lock().unwrap();
        assert!(notes[0].1.contains("2 commit(s):"));
        assert!(notes[0].1.contains("first"));
        assert!(notes[0].1.contains("second"));

        let _ = std::fs::remove_dir_all(&workdir);
    }

    #[tokio::test]
    async fn decide_review_auto_closes_automatically() {
        let tracker = MockTracker::default();
        let issue = TrackerIssue {
            id: "ENG-6".into(),
            title: "t".into(),
            description: None,
            decision_state: Some(DecisionState::Approved),
            review: ReviewMode::Auto,
        };
        let verdict = decide_review(
            &tracker,
            &issue,
            &[],
            &observability(),
            &std::env::temp_dir(),
            Duration::from_secs(5),
            None,
        )
        .await
        .unwrap();
        assert!(matches!(verdict, ReviewVerdict::CloseAutomatically));
    }

    #[tokio::test]
    async fn decide_review_human_needs_human() {
        let tracker = MockTracker::default();
        let issue = TrackerIssue {
            id: "ENG-7".into(),
            title: "t".into(),
            description: None,
            decision_state: Some(DecisionState::Approved),
            review: ReviewMode::Human,
        };
        let verdict = decide_review(
            &tracker,
            &issue,
            &[],
            &observability(),
            &std::env::temp_dir(),
            Duration::from_secs(5),
            None,
        )
        .await
        .unwrap();
        assert!(matches!(verdict, ReviewVerdict::NeedsHuman));
    }

    #[tokio::test]
    async fn finalize_completion_close_automatically_no_changes_marks_done() {
        let tracker = MockTracker::default();
        let issue = plain_issue("ENG-30", None);
        finalize_completion(
            &tracker,
            &issue,
            ReviewVerdict::CloseAutomatically,
            &std::env::temp_dir(),
            &PrOutcome::NoChanges,
        )
        .await
        .unwrap();
        assert_eq!(
            *tracker.decisions.lock().unwrap(),
            vec![("ENG-30".to_string(), DecisionState::Done)]
        );
    }

    #[tokio::test]
    async fn finalize_completion_needs_human_sets_needs_review() {
        let tracker = MockTracker::default();
        let issue = plain_issue("ENG-31", None);
        finalize_completion(
            &tracker,
            &issue,
            ReviewVerdict::NeedsHuman,
            &std::env::temp_dir(),
            &PrOutcome::NoChanges,
        )
        .await
        .unwrap();
        assert_eq!(
            *tracker.decisions.lock().unwrap(),
            vec![("ENG-31".to_string(), DecisionState::NeedsReview)]
        );
    }

    /// Regression test: a dispatch that committed real work but failed to push or
    /// open a PR for it (`PrOutcome::Failed`) must never read back as `Done` just
    /// because there's no PR to merge — that would silently drop the work with no
    /// review and no record of where it landed.
    #[tokio::test]
    async fn finalize_completion_pr_open_failure_needs_review_not_done() {
        let tracker = MockTracker::default();
        let issue = plain_issue("ENG-32", None);
        finalize_completion(
            &tracker,
            &issue,
            ReviewVerdict::CloseAutomatically,
            &std::env::temp_dir(),
            &PrOutcome::Failed,
        )
        .await
        .unwrap();
        assert_eq!(
            *tracker.decisions.lock().unwrap(),
            vec![("ENG-32".to_string(), DecisionState::NeedsReview)]
        );
    }

    /// `gh` isn't configured for a plain scratch repo with no remote, so `pr::merge`
    /// fails — exercising the "merge itself failed" path without needing real
    /// GitHub access. Must route to `NeedsReview`, never silently to `Done`.
    #[tokio::test]
    async fn finalize_completion_merge_failure_needs_review() {
        let repo_root =
            std::env::temp_dir().join(format!("lucid-mark-done-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&repo_root).unwrap();
        std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(&repo_root)
            .output()
            .unwrap();

        let tracker = MockTracker::default();
        let issue = plain_issue("ENG-33", None);
        let pr_outcome = PrOutcome::Created(PullRequest {
            url: "https://example.invalid/pr/1".to_string(),
            branch: "lucid/ENG-33".to_string(),
        });
        finalize_completion(
            &tracker,
            &issue,
            ReviewVerdict::CloseAutomatically,
            &repo_root,
            &pr_outcome,
        )
        .await
        .unwrap();
        assert_eq!(
            *tracker.decisions.lock().unwrap(),
            vec![("ENG-33".to_string(), DecisionState::NeedsReview)]
        );
        assert!(
            tracker.notes.lock().unwrap()[0]
                .1
                .contains("Could not merge PR")
        );

        let _ = std::fs::remove_dir_all(&repo_root);
    }

    #[test]
    fn parse_verdict_reads_pass_and_fail() {
        assert_eq!(parse_verdict("VERDICT: PASS").0, Some(true));
        assert_eq!(
            parse_verdict("some prose\nVERDICT: FAIL: missing tests").0,
            Some(false)
        );
        assert_eq!(parse_verdict("no verdict here").0, None);
    }

    fn plain_issue(id: &str, description: Option<&str>) -> TrackerIssue {
        TrackerIssue {
            id: id.to_string(),
            title: "t".into(),
            description: description.map(str::to_string),
            decision_state: Some(DecisionState::Approved),
            review: ReviewMode::Agent,
        }
    }

    #[test]
    fn resolve_verify_cmd_prefers_task_level_over_repo_default() {
        let issue = plain_issue("ENG-20", Some("---\nverify_cmd: \"task cmd\"\n---\n\nbody"));
        assert_eq!(
            resolve_verify_cmd(&issue, Some("repo cmd")),
            Some("task cmd".to_string())
        );
    }

    #[test]
    fn resolve_verify_cmd_falls_back_to_repo_default() {
        let issue = plain_issue("ENG-21", None);
        assert_eq!(
            resolve_verify_cmd(&issue, Some("repo cmd")),
            Some("repo cmd".to_string())
        );
    }

    #[test]
    fn resolve_verify_cmd_is_none_when_neither_tier_is_set() {
        let issue = plain_issue("ENG-22", None);
        assert_eq!(resolve_verify_cmd(&issue, None), None);
    }

    #[tokio::test]
    async fn decide_review_agent_verify_cmd_failure_skips_review_and_needs_human() {
        let tracker = MockTracker::default();
        let issue = TrackerIssue {
            id: "ENG-10".into(),
            title: "t".into(),
            description: Some("---\nverify_cmd: \"false\"\n---\n\nbody".into()),
            decision_state: Some(DecisionState::Approved),
            review: ReviewMode::Agent,
        };
        // Empty profiles: if verify_cmd's failure didn't short-circuit before the
        // LLM review dispatch, agent_review would hit DispatchError::NoProfiles and
        // this call would return Err — asserting Ok proves the review never ran.
        let verdict = decide_review(
            &tracker,
            &issue,
            &[],
            &observability(),
            &std::env::temp_dir(),
            Duration::from_secs(5),
            None,
        )
        .await
        .unwrap();
        assert!(matches!(verdict, ReviewVerdict::NeedsHuman));
        assert!(
            tracker.notes.lock().unwrap()[0]
                .1
                .contains("Verify command `false` failed")
        );
    }

    #[tokio::test]
    async fn decide_review_agent_verify_cmd_pass_then_llm_verdict_decides() {
        let tracker = MockTracker::default();
        let issue = TrackerIssue {
            id: "ENG-11".into(),
            title: "t".into(),
            description: Some("---\nverify_cmd: \"true\"\n---\n\nbody".into()),
            decision_state: Some(DecisionState::Approved),
            review: ReviewMode::Agent,
        };
        let event = serde_json::json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": "VERDICT: PASS",
        })
        .to_string();
        let profiles = [HarnessProfile {
            name: "fake-reviewer".into(),
            kind: HarnessKind::ClaudeCode,
            cmd: "sh".into(),
            args: vec!["-c".into(), format!("echo '{event}'")],
            auth_mode: AuthMode::Subscription,
            priority: 1,
        }];

        let verdict = decide_review(
            &tracker,
            &issue,
            &profiles,
            &observability(),
            &std::env::temp_dir(),
            Duration::from_secs(5),
            None,
        )
        .await
        .unwrap();
        assert!(matches!(verdict, ReviewVerdict::CloseAutomatically));
    }
}
