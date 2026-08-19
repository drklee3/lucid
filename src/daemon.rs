//! The reconciliation loop — dispatch of tracker-approved issues (presence-
//! independent: an `Approved` issue already cleared its human gate), plus
//! periodic, presence-gated PM wake cycles. See docs/FEATURES.md § Reconciliation
//! loop and
//! docs/wiki/architecture/observability.md (v1 is CLI-only: this loop prints its
//! own activity to stdout rather than serving a dashboard or an IPC status query).
//!
//! Deliberately sequential, not a concurrent task-supervisor: each tick dispatches
//! due work one issue at a time, fully awaited, and — per
//! docs/wiki/architecture/multi-project.md — projects are walked one at a time
//! too, not run concurrently. A true concurrent multi-worker loop (spawn + poll
//! across ticks, or parallel projects) is more machinery than this pass covers —
//! see the [2026-08-17] daemon-loop entry in docs/wiki/log.md. Stall protection
//! still works (`harness::DispatchRequest::timeout` kills a hung process), it
//! just means one very slow dispatch delays the next tick's other work rather
//! than running alongside it.
//!
//! Presence is resolved once per tick, globally — there's exactly one human
//! whose "away" state matters, so it doesn't vary by project. Everything else
//! (tracker binding, dispatch retry-tracking, PM-wake backoff) is per-project,
//! held on [`ProjectRuntime`].
//!
//! State (`runs`, PM-wake backoff timer, last presence mode) is persisted to a
//! flat JSON file after every tick, so a restart resumes rather than forgetting
//! in-flight tracking — same convention as `presence::override_file` and
//! `presence::audit_log` (see [`crate::state::DaemonState`]).

use crate::config::{Config, ObservabilityConfig, PresenceConfig, TrackerConfig};
use crate::harness::HarnessProfile;
use crate::pm;
use crate::pr;
use crate::presence::audit_log::AuditLog;
use crate::presence::override_file::OverrideFile;
use crate::presence::{self, PresenceMode, PresenceSourceList};
use crate::state::{DaemonState, ProjectId, WorkerRun};
use crate::tracker::{DecisionState, TrackerAdapter, TrackerIssue};
use crate::worker;
use crate::worktree;
use chrono::Utc;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

const PM_GOAL: &str = "keep the codebase healthy and close concrete, low-risk gaps";

/// Prefix on a blocked-issue note, checked against `list_comments` before
/// attaching another one — grepped for regardless of author, so it works the
/// same whether the comment came back rendered under lucid's own name (`FileTracker`)
/// or whatever Linear API-key user posted it (`LinearAdapter`). Keeps a still-blocked
/// issue from accumulating a duplicate note every tick.
const BLOCKED_NOTE_MARKER: &str = "[lucid:blocked]";

pub struct Daemon {
    profiles: Vec<HarnessProfile>,
    observability: ObservabilityConfig,
    presence_sources: PresenceSourceList,
    presence_cfg: PresenceConfig,
    override_file: OverrideFile,
    audit_log: AuditLog,
    worktree_root: PathBuf,
    tick_interval: Duration,
    stall_timeout: Duration,
    pm_wake_interval: Duration,
    last_mode: Mutex<Option<PresenceMode>>,
    state_path: PathBuf,
    /// Every repo this daemon instance watches — one entry when `[[projects]]`
    /// is empty (today's single-repo shape, wrapping `config.daemon.*`
    /// directly), one per pointer otherwise. See
    /// docs/wiki/architecture/multi-project.md.
    projects: Vec<ProjectRuntime>,
}

/// Per-project tracker binding, dispatch settings, and mutable run-tracking —
/// everything docs/wiki/architecture/multi-project.md calls out as becoming
/// per-project rather than staying global on `Daemon`. Presence detection,
/// harness profiles, observability, and tick/stall/PM-wake timing stay on
/// `Daemon` itself since those are genuinely shared across every project.
struct ProjectRuntime {
    /// This project's key into `DaemonState.runs`/`last_pm_wake`.
    project_id: ProjectId,
    tracker: Box<dyn TrackerAdapter>,
    workdir: PathBuf,
    base_branch: String,
    verify_cmd: Option<String>,
    runs: Mutex<HashMap<String, WorkerRun>>,
    last_pm_wake: Mutex<Option<chrono::DateTime<Utc>>>,
}

impl ProjectRuntime {
    fn new(
        project_id: ProjectId,
        tracker: Box<dyn TrackerAdapter>,
        workdir: PathBuf,
        base_branch: String,
        verify_cmd: Option<String>,
        loaded: &DaemonState,
    ) -> Self {
        let runs = loaded.runs.get(&project_id).cloned().unwrap_or_default();
        let last_pm_wake = loaded.last_pm_wake.get(&project_id).copied();
        Self {
            project_id,
            tracker,
            workdir,
            base_branch,
            verify_cmd,
            runs: Mutex::new(runs),
            last_pm_wake: Mutex::new(last_pm_wake),
        }
    }
}

/// Builds the effective per-project tracker config: `project_key` is the one
/// field `ProjectConfig` (repo-owned) and the global `[tracker]` block both
/// declare (see docs/wiki/architecture/multi-project.md's "Open, unresolved"
/// note) — a project's own declared key wins when set, falling back to the
/// operator's central `[tracker]` config otherwise. Everything else
/// (`backend`, `team_key`, `api_key_env`, `managed_label`) stays global: only
/// `project_key` is something a repo plausibly wants to declare for itself.
fn effective_tracker_config(base: &TrackerConfig, project_key: Option<String>) -> TrackerConfig {
    TrackerConfig {
        project_key: project_key.or_else(|| base.project_key.clone()),
        ..base.clone()
    }
}

impl Daemon {
    /// Builds every configured project's runtime — one, wrapping
    /// `config.daemon.*` directly, when `[[projects]]` is empty (today's
    /// single-repo shape, unchanged); one per pointer, each loading its own
    /// `lucid.project.toml` and building its own tracker, otherwise.
    ///
    /// # Errors
    /// Returns an error if a configured project's `lucid.project.toml` fails to
    /// load, or if building its tracker fails (e.g. a `linear` backend missing
    /// its API key env var).
    pub fn new(
        default_tracker: Box<dyn TrackerAdapter>,
        presence_sources: PresenceSourceList,
        config: &Config,
    ) -> anyhow::Result<Self> {
        let override_path = config
            .presence
            .override_path
            .clone()
            .unwrap_or_else(crate::config::default_override_path);
        let state_path = DaemonState::default_path();
        let loaded = DaemonState::load(&state_path);

        let projects = if config.projects.is_empty() {
            let project_id: ProjectId = config.daemon.workdir.to_string_lossy().into_owned();
            vec![ProjectRuntime::new(
                project_id,
                default_tracker,
                config.daemon.workdir.clone(),
                config.daemon.base_branch.clone(),
                config.daemon.verify_cmd.clone(),
                &loaded,
            )]
        } else {
            config
                .projects
                .iter()
                .map(|pointer| {
                    let project_config = crate::config::ProjectConfig::load(&pointer.path)
                        .map_err(|e| {
                            anyhow::anyhow!("project `{}`: {e}", pointer.path.display())
                        })?;
                    let tracker_config = effective_tracker_config(
                        &config.tracker,
                        project_config.project_key.clone(),
                    );
                    let tracker = crate::tracker::build(&tracker_config)?;
                    let project_id: ProjectId = pointer.path.to_string_lossy().into_owned();
                    let verify_cmd = project_config
                        .verify_cmd
                        .or_else(|| config.daemon.verify_cmd.clone());
                    Ok(ProjectRuntime::new(
                        project_id,
                        tracker,
                        pointer.path.clone(),
                        project_config.base_branch,
                        verify_cmd,
                        &loaded,
                    ))
                })
                .collect::<anyhow::Result<Vec<_>>>()?
        };

        Ok(Self {
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
            worktree_root: config.daemon.worktree_root.clone(),
            tick_interval: Duration::from_secs(config.daemon.tick_interval_secs),
            stall_timeout: Duration::from_secs(config.daemon.stall_timeout_secs),
            pm_wake_interval: Duration::from_secs(config.daemon.pm_wake_interval_mins * 60),
            last_mode: Mutex::new(loaded.last_mode),
            state_path,
            projects,
        })
    }

    /// Snapshots the current in-memory state and writes it to `state_path`, so a
    /// restart resumes from where the last tick left off.
    ///
    /// # Errors
    /// Returns an error if the parent directory can't be created or the file
    /// can't be written.
    fn save_state(&self) -> anyhow::Result<()> {
        let mut runs = HashMap::new();
        let mut last_pm_wake = HashMap::new();
        for project in &self.projects {
            runs.insert(
                project.project_id.clone(),
                project.runs.lock().unwrap().clone(),
            );
            if let Some(t) = *project.last_pm_wake.lock().unwrap() {
                last_pm_wake.insert(project.project_id.clone(), t);
            }
        }
        let state = DaemonState {
            runs,
            last_pm_wake,
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

    /// One reconciliation pass across every configured project: for each,
    /// reconcile any `NeedsReview` issues against their PR's merge status and
    /// dispatch any newly-approved issues — then resolve presence once, globally,
    /// and if autonomous, run a PM wake per project for any whose interval has
    /// elapsed.
    ///
    /// Projects are walked fully sequentially (see module doc) and a given
    /// project's reconcile/dispatch failure is logged and does not stop the
    /// remaining projects from getting their tick's work done; the first such
    /// error is still returned once every project (and presence/PM-wake) has
    /// been handled, so `run_foreground`'s caller-level log still fires.
    ///
    /// Presence only gates `maybe_wake_pm` — the PM proactively investigating and
    /// filing *new* proposals unsupervised. It does not gate dispatching an
    /// already-`Approved` issue: a human already reviewed and approved that
    /// specific ticket, so running it isn't unsupervised action the same way a PM
    /// wake is. See docs/wiki/architecture/presence-detection.md § What presence
    /// gates.
    async fn tick(&self) -> anyhow::Result<()> {
        let mut first_err = None;
        for project in &self.projects {
            if let Err(e) = self.tick_project(project).await {
                eprintln!("project `{}`: tick failed: {e}", project.project_id);
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }

        let idle_threshold =
            Duration::from_secs(u64::from(self.presence_cfg.idle_threshold_minutes) * 60);
        let mode =
            presence::resolve(&self.presence_sources, &self.override_file, idle_threshold).await?;

        let previous = self.last_mode.lock().unwrap().replace(mode);
        self.audit_log.record(previous, mode)?;

        if mode == PresenceMode::Autonomous {
            for project in &self.projects {
                self.maybe_wake_pm(project).await?;
            }
        }

        first_err.map_or(Ok(()), Err)
    }

    /// One project's reconcile-then-dispatch pass — the presence-independent
    /// half of a tick. Dispatch only runs if reconcile succeeded, matching the
    /// single-project sequencing this replaces; a failure here is caught by
    /// `tick`'s per-project loop rather than aborting other projects' work.
    async fn tick_project(&self, project: &ProjectRuntime) -> anyhow::Result<()> {
        self.reconcile_needs_review(project).await?;
        self.dispatch_approved_issues(project).await
    }

    /// Closes the loop on issues a human has already decided on GitHub: this only
    /// records a decision already made (via merge or close), it never dispatches
    /// new work — which is why it runs on every tick regardless of presence mode.
    async fn reconcile_needs_review(&self, project: &ProjectRuntime) -> anyhow::Result<()> {
        let needs_review = project
            .tracker
            .query_by_decision_state(DecisionState::NeedsReview)
            .await?;
        for issue in needs_review {
            let branch = worktree::branch_name(&issue.id);
            let status = pr::status(&project.workdir, &branch).await?;
            reconcile_issue(project.tracker.as_ref(), &issue, status).await?;
        }
        Ok(())
    }

    async fn dispatch_approved_issues(&self, project: &ProjectRuntime) -> anyhow::Result<()> {
        let approved = project
            .tracker
            .query_by_decision_state(DecisionState::Approved)
            .await?;
        for issue in approved {
            if skip_if_blocked(project.tracker.as_ref(), &issue).await? {
                println!("skipping {} — blocked", issue.id);
                continue;
            }

            // Dispatch on first sight, or retry a previous `Failed`/`TimedOut` run
            // — anything else (in particular `Succeeded`) is done and stays done.
            // No true in-flight tracking: ticks are sequential (see module doc),
            // so nothing is ever mid-run between ticks under this pass.
            let should_dispatch = {
                let runs = project.runs.lock().unwrap();
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
                project.tracker.as_ref(),
                &issue,
                &self.profiles,
                &self.observability,
                &project.workdir,
                &self.worktree_root,
                &project.base_branch,
                self.stall_timeout,
                project.verify_cmd.as_deref(),
            )
            .await?;
            println!("{} finished: {:?}", issue.id, run.phase);

            project.runs.lock().unwrap().insert(issue.id, run);
        }
        Ok(())
    }

    async fn maybe_wake_pm(&self, project: &ProjectRuntime) -> anyhow::Result<()> {
        let due = {
            let last = *project.last_pm_wake.lock().unwrap();
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

        println!("PM wake cycle starting for {}", project.project_id);
        // Record the attempt *before* the dispatch, regardless of outcome — a
        // failing wake must still back off for a full `pm_wake_interval`, or it
        // retries every tick forever. This is exactly the retry-storm pattern
        // flagged as the single most common real-world failure mode across every
        // system surveyed (docs/wiki/research/practitioner-reality.md); a PM wake
        // failure is logged and swallowed here rather than propagated as a fatal
        // tick error, since approved-issue dispatch above already succeeded and
        // shouldn't be treated as failed just because the PM step also ran.
        *project.last_pm_wake.lock().unwrap() = Some(Utc::now());

        match pm::wake(
            project.tracker.as_ref(),
            &self.profiles,
            &self.observability,
            &project.workdir,
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

    /// Snapshot of currently-known runs across every project, for `lucid status`
    /// when it's called in-process (e.g. a future IPC layer) — see docs/CLI.md §
    /// Not yet designed for why cross-process `status` isn't wired up yet.
    ///
    /// # Panics
    /// Panics if a project's internal run-state mutex is poisoned (a prior panic
    /// while holding the lock) — not expected in normal operation.
    #[must_use]
    pub fn runs_snapshot(&self) -> Vec<WorkerRun> {
        self.projects
            .iter()
            .flat_map(|p| p.runs.lock().unwrap().values().cloned().collect::<Vec<_>>())
            .collect()
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

/// Checks `issue`'s real tracker-native blockers and reports whether dispatch
/// should skip it this tick — any blocker not yet `Done` (including one with no
/// known decision state at all) counts as unresolved. On the first tick an issue
/// is found blocked, attaches a `BLOCKED_NOTE_MARKER` note naming the blocker(s);
/// on later ticks it stays blocked, `list_comments` already carries that marker
/// so no duplicate note goes out.
async fn skip_if_blocked(
    tracker: &dyn TrackerAdapter,
    issue: &TrackerIssue,
) -> anyhow::Result<bool> {
    let blockers = tracker.blockers(&issue.id).await?;
    let unresolved: Vec<&TrackerIssue> = blockers
        .iter()
        .filter(|b| b.decision_state != Some(DecisionState::Done))
        .collect();
    if unresolved.is_empty() {
        return Ok(false);
    }

    let already_noted = tracker
        .list_comments(&issue.id)
        .await?
        .iter()
        .any(|comment| comment.contains(BLOCKED_NOTE_MARKER));
    if !already_noted {
        let names = unresolved
            .iter()
            .map(|b| b.identifier.clone().unwrap_or_else(|| b.id.clone()))
            .collect::<Vec<_>>()
            .join(", ");
        let note = format!("{BLOCKED_NOTE_MARKER} waiting on: {names}");
        tracker.attach_note(&issue.id, &note).await?;
    }
    Ok(true)
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
        /// Every `DecisionState` queried, in call order — shared via `Arc` so a
        /// test can keep a handle after the tracker is boxed into a `Daemon`, to
        /// assert a query happened without needing a real dispatch/PM-wake path
        /// behind it.
        queried_states: std::sync::Arc<Mutex<Vec<DecisionState>>>,
        /// When set, `query_by_decision_state(Approved)` errors instead of
        /// returning — simulates one project's dispatch step failing, without
        /// needing a real worktree/harness dispatch behind it.
        fail_approved_query: bool,
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
            state: DecisionState,
        ) -> anyhow::Result<Vec<TrackerIssue>> {
            self.queried_states.lock().unwrap().push(state);
            if self.fail_approved_query && state == DecisionState::Approved {
                anyhow::bail!("simulated dispatch query failure");
            }
            Ok(Vec::new())
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
        async fn attach_link(
            &self,
            _issue_id: &str,
            _title: &str,
            _url: &str,
        ) -> anyhow::Result<()> {
            Ok(())
        }
        async fn list_comments(&self, _issue_id: &str) -> anyhow::Result<Vec<String>> {
            Ok(Vec::new())
        }
        async fn blockers(&self, _issue_id: &str) -> anyhow::Result<Vec<TrackerIssue>> {
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
            identifier: None,
        }
    }

    fn scratch_tracker(name: &str) -> crate::tracker::file::FileTracker {
        let path = std::env::temp_dir().join(format!(
            "lucid-daemon-blocked-test-{name}-{}.json",
            uuid::Uuid::new_v4()
        ));
        crate::tracker::file::FileTracker::open(&path).unwrap()
    }

    fn blocked_proposal(title: &str) -> Proposal {
        Proposal {
            title: title.to_string(),
            summary: "summary".to_string(),
            why_now: vec!["because".to_string()],
            effort_estimate: crate::tracker::EffortEstimate::Small,
            risk_note: "none".to_string(),
            task_type: "feature".to_string(),
            target_paths: vec![],
            acceptance_criteria: vec![],
            research_ref: None,
            review: ReviewMode::Auto,
            verify_cmd: None,
        }
    }

    #[tokio::test]
    async fn approved_issue_with_a_done_blocker_dispatches_normally() {
        let tracker = scratch_tracker("done-blocker");
        let blocker_id = tracker
            .create_proposal(&blocked_proposal("Blocker"))
            .await
            .unwrap();
        tracker
            .set_decision_state(&blocker_id, DecisionState::Done)
            .await
            .unwrap();
        let issue_id = tracker
            .create_proposal(&blocked_proposal("Blocked issue"))
            .await
            .unwrap();
        tracker.set_blockers(&issue_id, vec![blocker_id]).unwrap();

        let issue = tracker
            .query_by_decision_state(DecisionState::Pending)
            .await
            .unwrap()
            .into_iter()
            .find(|i| i.id == issue_id)
            .unwrap();

        let skipped = skip_if_blocked(&tracker, &issue).await.unwrap();
        assert!(!skipped);
        assert!(tracker.list_comments(&issue_id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn approved_issue_with_a_non_done_blocker_is_skipped_and_noted_once() {
        let tracker = scratch_tracker("pending-blocker");
        let blocker_id = tracker
            .create_proposal(&blocked_proposal("Blocker"))
            .await
            .unwrap();
        let issue_id = tracker
            .create_proposal(&blocked_proposal("Blocked issue"))
            .await
            .unwrap();
        tracker
            .set_blockers(&issue_id, vec![blocker_id.clone()])
            .unwrap();

        let issue = tracker
            .query_by_decision_state(DecisionState::Pending)
            .await
            .unwrap()
            .into_iter()
            .find(|i| i.id == issue_id)
            .unwrap();

        assert!(skip_if_blocked(&tracker, &issue).await.unwrap());
        let comments = tracker.list_comments(&issue_id).await.unwrap();
        assert_eq!(comments.len(), 1);
        assert!(comments[0].contains(BLOCKED_NOTE_MARKER));
        assert!(comments[0].contains(&blocker_id));

        // Second check on the same still-blocked issue must not add another note.
        assert!(skip_if_blocked(&tracker, &issue).await.unwrap());
        assert_eq!(tracker.list_comments(&issue_id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn issue_with_no_blockers_is_unaffected() {
        let tracker = scratch_tracker("no-blockers");
        let issue_id = tracker
            .create_proposal(&blocked_proposal("Standalone issue"))
            .await
            .unwrap();

        let issue = tracker
            .query_by_decision_state(DecisionState::Pending)
            .await
            .unwrap()
            .into_iter()
            .find(|i| i.id == issue_id)
            .unwrap();

        assert!(!skip_if_blocked(&tracker, &issue).await.unwrap());
        assert!(tracker.list_comments(&issue_id).await.unwrap().is_empty());
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

    /// Builds a `Daemon` directly (bypassing `Config`/`Daemon::new`) around the
    /// given trackers, one `ProjectRuntime` per tracker, with presence forced via
    /// the override file so tests don't depend on real idle sources. `unique`
    /// keeps each test's on-disk override/audit/state paths from colliding when
    /// tests run concurrently.
    fn test_daemon_multi(
        trackers: Vec<MockTracker>,
        override_mode: presence::override_file::OverrideMode,
        unique: &str,
    ) -> Daemon {
        let override_path =
            std::env::temp_dir().join(format!("lucid-daemon-test-override-{unique}"));
        let override_file = OverrideFile::new(override_path.clone());
        override_file.write(override_mode).unwrap();

        let projects = trackers
            .into_iter()
            .enumerate()
            .map(|(i, tracker)| ProjectRuntime {
                project_id: format!("test-project-{unique}-{i}"),
                tracker: Box::new(tracker),
                workdir: std::env::temp_dir(),
                base_branch: "main".to_string(),
                verify_cmd: None,
                runs: Mutex::new(HashMap::new()),
                last_pm_wake: Mutex::new(None),
            })
            .collect();

        Daemon {
            profiles: Vec::new(),
            observability: ObservabilityConfig {
                otlp_endpoint: "http://localhost:4317".to_string(),
                log_prompts: false,
                trace_ui_base_url: "http://localhost:6006".to_string(),
                trace_ui_project_id: None,
            },
            presence_sources: PresenceSourceList::new(Vec::new()),
            presence_cfg: PresenceConfig {
                idle_threshold_minutes: 20,
                proposal_cap_per_wake: 3,
                override_path: Some(override_path),
            },
            audit_log: AuditLog::new(
                std::env::temp_dir().join(format!("lucid-daemon-test-audit-{unique}")),
            ),
            override_file,
            worktree_root: std::env::temp_dir(),
            tick_interval: Duration::from_secs(60),
            stall_timeout: Duration::from_secs(5),
            pm_wake_interval: Duration::from_secs(3600),
            last_mode: Mutex::new(None),
            state_path: std::env::temp_dir().join(format!("lucid-daemon-test-state-{unique}.json")),
            projects,
        }
    }

    fn test_daemon(
        tracker: MockTracker,
        override_mode: presence::override_file::OverrideMode,
        unique: &str,
    ) -> Daemon {
        test_daemon_multi(vec![tracker], override_mode, unique)
    }

    #[tokio::test]
    async fn approved_issue_dispatch_is_not_gated_by_presence() {
        let tracker = MockTracker::default();
        let queried_states = tracker.queried_states.clone();
        let daemon = test_daemon(
            tracker,
            presence::override_file::OverrideMode::Active,
            "dispatch-not-gated",
        );

        daemon.tick().await.unwrap();

        assert_eq!(
            *daemon.last_mode.lock().unwrap(),
            Some(PresenceMode::Active)
        );
        // dispatch_approved_issues runs unconditionally: the tracker was queried
        // for Approved issues even though presence resolved to Active.
        assert!(
            queried_states
                .lock()
                .unwrap()
                .contains(&DecisionState::Approved)
        );
    }

    #[tokio::test]
    async fn pm_wake_does_not_fire_during_an_active_tick() {
        let daemon = test_daemon(
            MockTracker::default(),
            presence::override_file::OverrideMode::Active,
            "pm-wake-gated",
        );

        daemon.tick().await.unwrap();

        assert_eq!(
            *daemon.last_mode.lock().unwrap(),
            Some(PresenceMode::Active)
        );
        assert!(daemon.projects[0].last_pm_wake.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn tick_visits_every_configured_project() {
        let tracker_a = MockTracker::default();
        let tracker_b = MockTracker::default();
        let queried_a = tracker_a.queried_states.clone();
        let queried_b = tracker_b.queried_states.clone();
        let daemon = test_daemon_multi(
            vec![tracker_a, tracker_b],
            presence::override_file::OverrideMode::Active,
            "visits-every-project",
        );

        daemon.tick().await.unwrap();

        for queried in [&queried_a, &queried_b] {
            let states = queried.lock().unwrap();
            assert!(states.contains(&DecisionState::NeedsReview));
            assert!(states.contains(&DecisionState::Approved));
        }
    }

    #[tokio::test]
    async fn one_projects_dispatch_failure_does_not_block_the_other_projects_tick_work() {
        let failing_tracker = MockTracker {
            fail_approved_query: true,
            ..MockTracker::default()
        };
        let healthy_tracker = MockTracker::default();
        let healthy_queried = healthy_tracker.queried_states.clone();

        let daemon = test_daemon_multi(
            vec![failing_tracker, healthy_tracker],
            presence::override_file::OverrideMode::Active,
            "dispatch-failure-isolation",
        );

        // The first project's dispatch query fails, but the tick as a whole still
        // surfaces that failure once every project's work (and presence
        // resolution) has run.
        assert!(daemon.tick().await.is_err());

        let states = healthy_queried.lock().unwrap();
        assert!(states.contains(&DecisionState::NeedsReview));
        assert!(states.contains(&DecisionState::Approved));
        // Presence still resolved despite the first project's failure.
        assert_eq!(
            *daemon.last_mode.lock().unwrap(),
            Some(PresenceMode::Active)
        );
    }
}
