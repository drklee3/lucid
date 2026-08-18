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
use crate::state::{ClaimState, ClaimedSubstate, WorkerPhase, WorkerRun};
use crate::tracker::{DecisionState, ReviewMode, TrackerAdapter, TrackerIssue};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// How a successful dispatch's changes get committed — see
/// docs/wiki/architecture/worker-completion.md. There is no `BranchAndPr` mode:
/// with no per-issue worktree isolation yet, dispatch runs directly in
/// `daemon.workdir`, so "land on `main` locally" is the only thing lucid can do
/// without either scooping up unrelated files in a blind `git add -A` or building
/// worktree management first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CompletionMode {
    /// Today's behavior: lucid doesn't touch git at all. Whatever the harness
    /// did (or didn't) to the working tree is left as-is.
    #[default]
    None,
    /// The dispatch prompt instructs the harness to commit its own work — it has
    /// full `auto` tool access and knows its own diff, so the commit message is
    /// better than anything lucid could synthesize post-hoc. lucid never runs
    /// `git add`/`git commit` itself: in a shared, non-worktree-isolated
    /// `workdir`, a blind `git add -A` can't tell the harness's changes apart
    /// from unrelated in-progress files. lucid only *observes* the result
    /// (`HEAD` before/after, `git status --porcelain`) and reports it.
    Commit,
}

/// Builds the dispatch prompt for a claimed issue: title as a heading, plus the
/// frontmatter+body handoff surface (see docs/wiki/architecture/agent-handoff.md)
/// when the tracker has one, plus a commit instruction under `CompletionMode::Commit`.
/// Falls back to the bare title for issues created outside `create_proposal` (e.g.
/// hand-filed tracker items) — the Worker still dispatches, just with less to go on.
#[must_use]
pub fn dispatch_prompt(issue: &TrackerIssue, completion_mode: CompletionMode) -> String {
    let mut prompt = match &issue.description {
        Some(description) => format!("# {}\n\n{description}", issue.title),
        None => issue.title.clone(),
    };
    if completion_mode == CompletionMode::Commit {
        use std::fmt::Write;
        let _ = write!(
            prompt,
            "\n\n---\n\nWhen you're done and confident any acceptance criteria above are \
             met, commit your changes yourself with `git commit` — one commit or several, \
             whatever's logically right for the change; include `{}` in at least one commit \
             message. Do not push, and do not open a pull request. If there's nothing worth \
             committing, leave the working tree as it is.",
            issue.id
        );
    }
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
/// `git` isn't on `PATH`) — non-fatal either way, since `CompletionMode::Commit`
/// is opt-in and a misconfigured `workdir` shouldn't crash the dispatch.
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

/// Describes what happened to the working tree under `CompletionMode::Commit`, by
/// listing commits made since dispatch started — lucid never runs a git-mutating
/// command itself here (see `CompletionMode::Commit`'s doc comment for why).
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
            Some(n) => format!(
                "Left {n} file(s) uncommitted — lucid doesn't auto-commit (see CompletionMode::Commit)."
            ),
            None => "Commit status unknown (not a git repository, or `git` isn't available)."
                .to_string(),
        },
        None => "Commit status unknown (not a git repository, or `git` isn't available)."
            .to_string(),
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
    completion_mode: CompletionMode,
) -> anyhow::Result<WorkerRun> {
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

    let head_before = if completion_mode == CompletionMode::Commit {
        git_head(workdir).await
    } else {
        None
    };

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
            if completion_mode == CompletionMode::Commit {
                use std::fmt::Write;
                let commit_status = describe_commit_result(workdir, head_before.as_deref()).await;
                let _ = write!(note, "\n{commit_status}");
            }
            note
        }
        Err(e) => {
            // A timeout is a distinct, retriable state (`TimedOut`), not a plain
            // task failure — the reconciliation loop's retry policy treats these
            // differently (see docs/FEATURES.md § Reconciliation loop).
            run.phase = if matches!(e.downcast_ref::<DispatchError>(), Some(DispatchError::Timeout { .. })) {
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

/// Dispatches one already-`Approved` issue end to end: builds the prompt, runs the
/// harness, and finalizes the tracker's decision state per `issue.review`. Shared by
/// `daemon::dispatch_approved_issues` (the regular presence-gated tick) and
/// `lucid task dispatch-now` (an on-demand trigger of the *exact same* path, not a
/// separate one — see docs/wiki/architecture/worker-completion.md).
///
/// # Errors
/// Returns an error if `run_dispatch` or `finalize_completion` does (tracker calls
/// failing, not a normal dispatch failure — see their own docs).
#[allow(clippy::too_many_arguments)]
pub async fn dispatch_and_finalize(
    tracker: &dyn TrackerAdapter,
    issue: &TrackerIssue,
    profiles: &[HarnessProfile],
    observability: &ObservabilityConfig,
    workdir: &Path,
    stall_timeout: Duration,
    completion_mode: CompletionMode,
) -> anyhow::Result<WorkerRun> {
    let prompt = dispatch_prompt(issue, completion_mode);
    let run = run_dispatch(
        tracker,
        &issue.id,
        &prompt,
        profiles,
        observability,
        workdir,
        stall_timeout,
        completion_mode,
    )
    .await?;
    finalize_completion(tracker, issue, &run, profiles, observability, workdir, stall_timeout).await?;
    Ok(run)
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

/// Decides what a successful dispatch means for the tracker item — the completion
/// half of "how does a Worker finish a task" (see
/// docs/wiki/architecture/worker-completion.md). Never called for a `Failed`/
/// `TimedOut` run: the issue's `DecisionState` stays `Approved` so the daemon's
/// existing retry path (`daemon::dispatch_approved_issues`) picks it back up.
///
/// # Errors
/// Returns an error if a tracker call fails, or (for `ReviewMode::Agent`) if the
/// review dispatch itself fails to even run — a review that ran but returned an
/// unparseable verdict is *not* an error, it routes to `NeedsReview` instead.
pub async fn finalize_completion(
    tracker: &dyn TrackerAdapter,
    issue: &TrackerIssue,
    run: &WorkerRun,
    profiles: &[HarnessProfile],
    observability: &ObservabilityConfig,
    workdir: &Path,
    stall_timeout: Duration,
) -> anyhow::Result<()> {
    if run.phase != WorkerPhase::Succeeded {
        return Ok(());
    }

    match issue.review {
        ReviewMode::Auto => tracker.set_decision_state(&issue.id, DecisionState::Done).await,
        ReviewMode::Human => {
            tracker
                .set_decision_state(&issue.id, DecisionState::NeedsReview)
                .await
        }
        ReviewMode::Agent => {
            let (verdict, note) =
                agent_review(issue, profiles, observability, workdir, stall_timeout).await?;
            tracker.attach_note(&issue.id, &note).await?;
            let state = if verdict == Some(true) {
                DecisionState::Done
            } else {
                DecisionState::NeedsReview
            };
            tracker.set_decision_state(&issue.id, state).await
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
        async fn set_decision_state(&self, issue_id: &str, state: DecisionState) -> anyhow::Result<()> {
            self.decisions
                .lock()
                .unwrap()
                .push((issue_id.to_string(), state));
            Ok(())
        }
        async fn query_by_label(&self, _label: &str) -> anyhow::Result<Vec<TrackerIssue>> {
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
        let prompt = dispatch_prompt(&issue, CompletionMode::None);
        assert!(prompt.starts_with("# Fix presence detection\n\n"));
        assert!(prompt.contains("IdleHint never resets."));
    }

    #[test]
    fn dispatch_prompt_falls_back_to_the_title_without_a_description() {
        let issue = TrackerIssue {
            id: "ENG-2".into(),
            title: "Hand-filed issue".into(),
            description: None,
            decision_state: None,
            review: crate::tracker::ReviewMode::Auto,
        };
        assert_eq!(dispatch_prompt(&issue, CompletionMode::None), "Hand-filed issue");
    }

    #[test]
    fn dispatch_prompt_appends_a_commit_instruction_under_commit_mode() {
        let issue = TrackerIssue {
            id: "ENG-4".into(),
            title: "Hand-filed issue".into(),
            description: None,
            decision_state: None,
            review: crate::tracker::ReviewMode::Auto,
        };
        let prompt = dispatch_prompt(&issue, CompletionMode::Commit);
        assert!(prompt.contains("git commit"));
        assert!(prompt.contains("ENG-4"));
        assert!(prompt.contains("Do not push"));
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
            CompletionMode::None,
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
            CompletionMode::None,
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
            CompletionMode::None,
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
    async fn commit_mode_reports_a_new_commit_in_the_note() {
        let workdir = std::env::temp_dir().join(format!("lucid-commit-mode-test-{}", uuid::Uuid::new_v4()));
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
        // makes the commit that a real dispatch under `CompletionMode::Commit`
        // would have made — the point under test is `run_dispatch` *observing* it
        // via `HEAD` before/after, not the (already-covered) prompt instruction.
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
            CompletionMode::Commit,
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
    async fn commit_mode_reports_every_commit_not_just_the_last() {
        let workdir = std::env::temp_dir().join(format!("lucid-multi-commit-test-{}", uuid::Uuid::new_v4()));
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
            CompletionMode::Commit,
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
    async fn finalize_completion_auto_marks_done() {
        let tracker = MockTracker::default();
        let issue = TrackerIssue {
            id: "ENG-6".into(),
            title: "t".into(),
            description: None,
            decision_state: Some(DecisionState::Approved),
            review: ReviewMode::Auto,
        };
        let run = WorkerRun {
            issue_id: "ENG-6".into(),
            claim: ClaimState::Released,
            phase: WorkerPhase::Succeeded,
            session_id: None,
            dispatch_id: None,
            retries: 0,
            last_event_at: Utc::now(),
            last_error: None,
        };
        finalize_completion(
            &tracker,
            &issue,
            &run,
            &[],
            &observability(),
            &std::env::temp_dir(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(*tracker.decisions.lock().unwrap(), vec![("ENG-6".to_string(), DecisionState::Done)]);
    }

    #[tokio::test]
    async fn finalize_completion_human_needs_review() {
        let tracker = MockTracker::default();
        let issue = TrackerIssue {
            id: "ENG-7".into(),
            title: "t".into(),
            description: None,
            decision_state: Some(DecisionState::Approved),
            review: ReviewMode::Human,
        };
        let run = WorkerRun {
            issue_id: "ENG-7".into(),
            claim: ClaimState::Released,
            phase: WorkerPhase::Succeeded,
            session_id: None,
            dispatch_id: None,
            retries: 0,
            last_event_at: Utc::now(),
            last_error: None,
        };
        finalize_completion(
            &tracker,
            &issue,
            &run,
            &[],
            &observability(),
            &std::env::temp_dir(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(
            *tracker.decisions.lock().unwrap(),
            vec![("ENG-7".to_string(), DecisionState::NeedsReview)]
        );
    }

    #[tokio::test]
    async fn finalize_completion_skips_non_succeeded_runs() {
        let tracker = MockTracker::default();
        let issue = TrackerIssue {
            id: "ENG-8".into(),
            title: "t".into(),
            description: None,
            decision_state: Some(DecisionState::Approved),
            review: ReviewMode::Auto,
        };
        let run = WorkerRun {
            issue_id: "ENG-8".into(),
            claim: ClaimState::Released,
            phase: WorkerPhase::Failed,
            session_id: None,
            dispatch_id: None,
            retries: 0,
            last_event_at: Utc::now(),
            last_error: None,
        };
        finalize_completion(
            &tracker,
            &issue,
            &run,
            &[],
            &observability(),
            &std::env::temp_dir(),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert!(tracker.decisions.lock().unwrap().is_empty());
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
}
