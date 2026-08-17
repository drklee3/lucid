//! Wires one dispatch attempt to its tracker item: run the harness, then post the
//! resulting trace link back as a proof-of-work artifact regardless of outcome —
//! see docs/wiki/architecture/trace-correlation.md and
//! docs/wiki/architecture/observability.md#proof-of-work-artifacts.
//!
//! This is deliberately just the dispatch-to-tracker wiring, not the reconciliation
//! loop that decides *when* to call it (stall-detect, retry policy, presence
//! gating) — that's a separate, larger piece (see docs/FEATURES.md § Reconciliation
//! loop) not built yet.

use crate::config::ObservabilityConfig;
use crate::harness::{self, DispatchError, DispatchRequest, HarnessProfile, TelemetryConfig};
use crate::state::{ClaimState, ClaimedSubstate, WorkerPhase, WorkerRun};
use crate::tracker::TrackerAdapter;
use chrono::Utc;
use std::path::Path;
use std::time::Duration;

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
            format!(
                "Dispatch `{}` via `{}` — status: {status_desc}\nTrace: {link}",
                o.dispatch_id, o.profile_name
            )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{AuthMode, HarnessKind};
    use crate::tracker::{DecisionState, Proposal, TrackerIssue};
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// Records every note posted, in place of a real tracker backend — none of
    /// Linear's actual GraphQL calls are implemented yet.
    #[derive(Default)]
    struct MockTracker {
        notes: Mutex<Vec<(String, String)>>,
    }

    #[async_trait]
    impl TrackerAdapter for MockTracker {
        async fn create_proposal(&self, _proposal: &Proposal) -> anyhow::Result<String> {
            unimplemented!("not exercised by these tests")
        }
        async fn set_decision_state(
            &self,
            _issue_id: &str,
            _state: DecisionState,
        ) -> anyhow::Result<()> {
            unimplemented!("not exercised by these tests")
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
}
