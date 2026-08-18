//! The reconciliation loop — presence-gated dispatch of tracker-approved issues,
//! plus periodic PM wake cycles. See docs/FEATURES.md § Reconciliation loop and
//! docs/wiki/architecture/observability.md (v1 is CLI-only: this loop prints its
//! own activity to stdout rather than serving a dashboard or an IPC status query).
//!
//! Deliberately sequential, not a concurrent task-supervisor: each tick dispatches
//! due work one issue at a time, fully awaited. A true concurrent multi-worker
//! loop (spawn + poll across ticks) is more machinery than this pass covers — see
//! the [2026-08-17] daemon-loop entry in docs/wiki/log.md. Stall protection still
//! works (`harness::DispatchRequest::timeout` kills a hung process), it just means
//! one very slow dispatch delays the next tick's other work rather than running
//! alongside it.
//!
//! State (`runs`, PM-wake backoff timer, last presence mode) is persisted to a
//! flat JSON file after every tick, so a restart resumes rather than forgetting
//! in-flight tracking — same convention as `presence::override_file` and
//! `presence::audit_log` (see [`crate::state::DaemonState`]).

use crate::config::{Config, ObservabilityConfig, PresenceConfig};
use crate::harness::HarnessProfile;
use crate::pm;
use crate::pr;
use crate::presence::audit_log::AuditLog;
use crate::presence::override_file::OverrideFile;
use crate::presence::{self, PresenceMode, PresenceSourceList};
use crate::state::{DaemonState, WorkerRun};
use crate::tracker::{DecisionState, TrackerAdapter, TrackerIssue};
use crate::worker;
use crate::worktree;
use chrono::Utc;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

const PM_GOAL: &str = "keep the codebase healthy and close concrete, low-risk gaps";

pub struct Daemon {
    tracker: Box<dyn TrackerAdapter>,
    profiles: Vec<HarnessProfile>,
    observability: ObservabilityConfig,
    presence_sources: PresenceSourceList,
    presence_cfg: PresenceConfig,
    override_file: OverrideFile,
    audit_log: AuditLog,
    workdir: PathBuf,
    base_branch: String,
    worktree_root: PathBuf,
    verify_cmd: Option<String>,
    tick_interval: Duration,
    stall_timeout: Duration,
    pm_wake_interval: Duration,
    runs: Mutex<HashMap<String, WorkerRun>>,
    last_pm_wake: Mutex<Option<chrono::DateTime<Utc>>>,
    last_mode: Mutex<Option<PresenceMode>>,
    state_path: PathBuf,
}

impl Daemon {
    #[must_use]
    pub fn new(
        tracker: Box<dyn TrackerAdapter>,
        presence_sources: PresenceSourceList,
        config: &Config,
    ) -> Self {
        let override_path = config
            .presence
            .override_path
            .clone()
            .unwrap_or_else(crate::config::default_override_path);
        let state_path = DaemonState::default_path();
        let loaded = DaemonState::load(&state_path);
        Self {
            tracker,
            profiles: config.harness_profiles.clone(),
            observability: ObservabilityConfig {
                otlp_endpoint: config.observability.otlp_endpoint.clone(),
                log_prompts: config.observability.log_prompts,
                trace_ui_base_url: config.observability.trace_ui_base_url.clone(),
                trace_ui_project_id: config.observability.trace_ui_project_id.clone(),
            },
            presence_sources,
            presence_cfg: PresenceConfig {
                idle_threshold_minutes: config.presence.idle_threshold_minutes,
                proposal_cap_per_wake: config.presence.proposal_cap_per_wake,
                override_path: Some(override_path.clone()),
            },
            audit_log: AuditLog::new(AuditLog::default_path_from_override(&override_path)),
            override_file: OverrideFile::new(override_path),
            workdir: config.daemon.workdir.clone(),
            base_branch: config.daemon.base_branch.clone(),
            worktree_root: config.daemon.worktree_root.clone(),
            verify_cmd: config.daemon.verify_cmd.clone(),
            tick_interval: Duration::from_secs(config.daemon.tick_interval_secs),
            stall_timeout: Duration::from_secs(config.daemon.stall_timeout_secs),
            pm_wake_interval: Duration::from_secs(config.daemon.pm_wake_interval_mins * 60),
            runs: Mutex::new(loaded.runs),
            last_pm_wake: Mutex::new(loaded.last_pm_wake),
            last_mode: Mutex::new(loaded.last_mode),
            state_path,
        }
    }

    /// Snapshots the current in-memory state and writes it to `state_path`, so a
    /// restart resumes from where the last tick left off.
    ///
    /// # Errors
    /// Returns an error if the parent directory can't be created or the file
    /// can't be written.
    fn save_state(&self) -> anyhow::Result<()> {
        let state = DaemonState {
            runs: self.runs.lock().unwrap().clone(),
            last_pm_wake: *self.last_pm_wake.lock().unwrap(),
            last_mode: *self.last_mode.lock().unwrap(),
        };
        state.save(&self.state_path)
    }

    /// Runs until Ctrl-C. Foreground only — v1 has no detach/IPC story (see
    /// docs/CLI.md § Not yet designed).
    ///
    /// # Errors
    /// Only returns `Err` for a `tokio::signal::ctrl_c` setup failure; a failed
    /// individual tick is logged to stdout and the loop continues.
    pub async fn run_foreground(&self) -> anyhow::Result<()> {
        println!(
            "lucid daemon starting — tick every {:?}",
            self.tick_interval
        );
        loop {
            tokio::select! {
                result = tokio::signal::ctrl_c() => {
                    result?;
                    println!("shutdown requested, exiting");
                    return Ok(());
                }
                () = tokio::time::sleep(self.tick_interval) => {
                    if let Err(e) = self.tick().await {
                        eprintln!("reconciliation tick failed: {e}");
                    }
                    if let Err(e) = self.save_state() {
                        eprintln!("failed to persist daemon state: {e}");
                    }
                }
            }
        }
    }

    /// One reconciliation pass: reconcile any `NeedsReview` issues against their
    /// PR's merge status, resolve presence, and if autonomous, dispatch any
    /// newly-approved issues plus run a PM wake if its interval has elapsed.
    async fn tick(&self) -> anyhow::Result<()> {
        self.reconcile_needs_review().await?;

        let idle_threshold =
            Duration::from_secs(u64::from(self.presence_cfg.idle_threshold_minutes) * 60);
        let mode =
            presence::resolve(&self.presence_sources, &self.override_file, idle_threshold).await?;

        let previous = self.last_mode.lock().unwrap().replace(mode);
        self.audit_log.record(previous, mode)?;

        if mode != PresenceMode::Autonomous {
            return Ok(());
        }

        self.dispatch_approved_issues().await?;
        self.maybe_wake_pm().await?;
        Ok(())
    }

    /// Closes the loop on issues a human has already decided on GitHub: this only
    /// records a decision already made (via merge or close), it never dispatches
    /// new work — which is why it runs on every tick regardless of presence mode.
    async fn reconcile_needs_review(&self) -> anyhow::Result<()> {
        let needs_review = self
            .tracker
            .query_by_decision_state(DecisionState::NeedsReview)
            .await?;
        for issue in needs_review {
            let branch = worktree::branch_name(&issue.id);
            let status = pr::status(&self.workdir, &branch).await?;
            reconcile_issue(self.tracker.as_ref(), &issue, status).await?;
        }
        Ok(())
    }

    async fn dispatch_approved_issues(&self) -> anyhow::Result<()> {
        let approved = self
            .tracker
            .query_by_decision_state(DecisionState::Approved)
            .await?;
        for issue in approved {
            // Dispatch on first sight, or retry a previous `Failed`/`TimedOut` run
            // — anything else (in particular `Succeeded`) is done and stays done.
            // No true in-flight tracking: ticks are sequential (see module doc),
            // so nothing is ever mid-run between ticks under this pass.
            let should_dispatch = {
                let runs = self.runs.lock().unwrap();
                runs.get(&issue.id).is_none_or(|prev| {
                    matches!(
                        prev.phase,
                        crate::state::WorkerPhase::Failed | crate::state::WorkerPhase::TimedOut
                    )
                })
            };
            if !should_dispatch {
                continue;
            }

            println!("dispatching {} — {}", issue.id, issue.title);
            // Also decides Done / NeedsReview per `issue.review` — a no-op for a
            // Failed/TimedOut run, which leaves the tracker item `Approved` so the
            // `should_dispatch` check above retries it on a later tick.
            let run = worker::dispatch_and_finalize(
                self.tracker.as_ref(),
                &issue,
                &self.profiles,
                &self.observability,
                &self.workdir,
                &self.worktree_root,
                &self.base_branch,
                self.stall_timeout,
                self.verify_cmd.as_deref(),
            )
            .await?;
            println!("{} finished: {:?}", issue.id, run.phase);

            self.runs.lock().unwrap().insert(issue.id, run);
        }
        Ok(())
    }

    async fn maybe_wake_pm(&self) -> anyhow::Result<()> {
        let due = {
            let last = *self.last_pm_wake.lock().unwrap();
            last.is_none_or(|t| {
                Utc::now()
                    .signed_duration_since(t)
                    .to_std()
                    .unwrap_or_default()
                    >= self.pm_wake_interval
            })
        };
        if !due {
            return Ok(());
        }

        println!("PM wake cycle starting");
        // Record the attempt *before* the dispatch, regardless of outcome — a
        // failing wake must still back off for a full `pm_wake_interval`, or it
        // retries every tick forever. This is exactly the retry-storm pattern
        // flagged as the single most common real-world failure mode across every
        // system surveyed (docs/wiki/research/practitioner-reality.md); a PM wake
        // failure is logged and swallowed here rather than propagated as a fatal
        // tick error, since approved-issue dispatch above already succeeded and
        // shouldn't be treated as failed just because the PM step also ran.
        *self.last_pm_wake.lock().unwrap() = Some(Utc::now());

        match pm::wake(
            self.tracker.as_ref(),
            &self.profiles,
            &self.observability,
            &self.workdir,
            PM_GOAL,
            self.presence_cfg.proposal_cap_per_wake,
            self.stall_timeout,
            false,
        )
        .await
        {
            Ok(outcome) => println!(
                "PM wake filed {} proposal(s), skipped {} as similar to existing issues",
                outcome.filed.len(),
                outcome.skipped_similar.len()
            ),
            Err(e) => eprintln!(
                "PM wake failed (will retry after {:?}): {e}",
                self.pm_wake_interval
            ),
        }
        Ok(())
    }

    /// Snapshot of currently-known runs, for `lucid status` when it's called
    /// in-process (e.g. a future IPC layer) — see docs/CLI.md § Not yet designed
    /// for why cross-process `status` isn't wired up yet.
    ///
    /// # Panics
    /// Panics if the internal run-state mutex is poisoned (a prior panic while
    /// holding the lock) — not expected in normal operation.
    #[must_use]
    pub fn runs_snapshot(&self) -> Vec<WorkerRun> {
        self.runs.lock().unwrap().values().cloned().collect()
    }
}

/// Applies a `NeedsReview` issue's already-looked-up PR status. `Merged` closes
/// the loop as `Done`; `Closed` (without merging) as `Rejected`. `Open` or `None`
/// (no PR found for this branch yet) leaves the issue untouched — there's no
/// human decision yet to record.
async fn reconcile_issue(
    tracker: &dyn TrackerAdapter,
    issue: &TrackerIssue,
    status: Option<pr::PrStatus>,
) -> anyhow::Result<()> {
    let (state, note) = match status {
        Some(pr::PrStatus::Merged) => (
            DecisionState::Done,
            "PR merged on GitHub — reconciled to Done.",
        ),
        Some(pr::PrStatus::Closed) => (
            DecisionState::Rejected,
            "PR closed without merging on GitHub — reconciled to Rejected.",
        ),
        Some(pr::PrStatus::Open) | None => return Ok(()),
    };
    tracker.attach_note(&issue.id, note).await?;
    tracker.set_decision_state(&issue.id, state).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::{Proposal, ReviewMode};
    use async_trait::async_trait;
    use std::sync::Mutex;

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
        async fn list_comments(&self, _issue_id: &str) -> anyhow::Result<Vec<String>> {
            Ok(Vec::new())
        }
    }

    fn issue() -> TrackerIssue {
        TrackerIssue {
            id: "ENG-9".into(),
            title: "Something needing review".into(),
            description: None,
            decision_state: Some(DecisionState::NeedsReview),
            review: ReviewMode::Agent,
        }
    }

    #[tokio::test]
    async fn merged_pr_marks_the_issue_done() {
        let tracker = MockTracker::default();
        reconcile_issue(&tracker, &issue(), Some(pr::PrStatus::Merged))
            .await
            .unwrap();

        assert_eq!(
            *tracker.decisions.lock().unwrap(),
            vec![("ENG-9".to_string(), DecisionState::Done)]
        );
        assert_eq!(tracker.notes.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn closed_without_merge_marks_the_issue_rejected() {
        let tracker = MockTracker::default();
        reconcile_issue(&tracker, &issue(), Some(pr::PrStatus::Closed))
            .await
            .unwrap();

        assert_eq!(
            *tracker.decisions.lock().unwrap(),
            vec![("ENG-9".to_string(), DecisionState::Rejected)]
        );
        assert_eq!(tracker.notes.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn open_pr_is_left_untouched() {
        let tracker = MockTracker::default();
        reconcile_issue(&tracker, &issue(), Some(pr::PrStatus::Open))
            .await
            .unwrap();

        assert!(tracker.decisions.lock().unwrap().is_empty());
        assert!(tracker.notes.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn no_pr_found_is_left_untouched() {
        let tracker = MockTracker::default();
        reconcile_issue(&tracker, &issue(), None).await.unwrap();

        assert!(tracker.decisions.lock().unwrap().is_empty());
        assert!(tracker.notes.lock().unwrap().is_empty());
    }
}
