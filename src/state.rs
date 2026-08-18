//! Worker and orchestration state machines.
//!
//! Exhaustive `match` on these enums is the whole point of choosing Rust for this
//! project (see docs/wiki/architecture/tech-stack.md) — adding a state and
//! forgetting to handle it somewhere becomes a compile error, not a live bug
//! discovered the way `OpenHands`', cyrus's, and Symphony's own state gaps were.

use crate::presence::PresenceMode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Orchestration state, separate from whatever the tracker calls its own statuses
/// (Symphony's pattern — see docs/wiki/architecture/symphony-patterns.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClaimState {
    Unclaimed,
    Claimed(ClaimedSubstate),
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClaimedSubstate {
    Running,
    RetryQueued,
}

/// Per-run Worker phase. Symphony's eleven phases, plus two states nothing surveyed
/// had (see docs/wiki/architecture/state-machine-gaps.md):
///
/// - `AwaitingHumanInput`: the Worker paused mid-task to ask a clarifying question
///   (Linear Agent Sessions' `awaitingInput`/`elicitation` pattern — we had no
///   equivalent before this).
/// - `Stuck`: detected unproductive looping (busywork, not silence) — distinct from
///   `Stalled`, which is purely `elapsed_ms > stall_timeout_ms`. `OpenHands` has this
///   as a first-class state; Symphony does not. The *detection logic* for this is
///   explicitly not designed yet (docs/FEATURES.md, Deferred) — only the state
///   exists so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerPhase {
    PreparingWorkspace,
    BuildingPrompt,
    LaunchingAgentProcess,
    InitializingSession,
    StreamingTurn,
    AwaitingHumanInput,
    Finishing,
    Succeeded,
    Failed,
    TimedOut,
    Stalled,
    Stuck,
    CanceledByReconciliation,
}

impl WorkerPhase {
    /// A run is done once it lands in one of these — no further reconciliation-tick
    /// work applies to it beyond cleanup.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            WorkerPhase::Succeeded
                | WorkerPhase::Failed
                | WorkerPhase::TimedOut
                | WorkerPhase::CanceledByReconciliation
        )
    }

    /// Symphony's "parked state stops polling" rule (see
    /// docs/wiki/architecture/state-machine-gaps.md), made explicit here rather
    /// than left implicit: once a Worker needs a human, the reconciliation tick
    /// should stop actively dispatching/polling it.
    #[must_use]
    pub fn is_parked_for_human(self) -> bool {
        matches!(self, WorkerPhase::AwaitingHumanInput)
    }
}

/// One tracked run of a Worker against a single tracker item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerRun {
    pub issue_id: String,
    pub claim: ClaimState,
    pub phase: WorkerPhase,
    pub session_id: Option<String>,
    /// Correlation id for the dispatch attempt behind this run — the same value
    /// tagged onto the harness's `OTel` traces via `lucid.dispatch_id`. A fresh id
    /// per attempt, so retries stay distinguishable in the trace store (see
    /// docs/wiki/architecture/trace-correlation.md).
    pub dispatch_id: Option<String>,
    pub retries: u32,
    pub last_event_at: DateTime<Utc>,
    pub last_error: Option<String>,
}

/// Daemon state persisted across restarts — same flat-file convention as
/// `presence::override_file` and `presence::audit_log` rather than a database
/// (see docs/wiki/architecture/persistence.md).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DaemonState {
    pub runs: HashMap<String, WorkerRun>,
    pub last_pm_wake: Option<DateTime<Utc>>,
    pub last_mode: Option<PresenceMode>,
}

impl DaemonState {
    /// Resolved the same way as `config::default_override_path`:
    /// `$XDG_STATE_HOME/lucid`, falling back to `~/.local/state/lucid`.
    #[must_use]
    pub fn default_path() -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
            return PathBuf::from(xdg).join("lucid/daemon-state.json");
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home).join(".local/state/lucid/daemon-state.json");
        }
        PathBuf::from(".lucid-daemon-state.json")
    }

    /// Missing, unreadable, or unparseable file reads as a fresh empty state —
    /// matches `OverrideFile::read`'s tolerance for a missing file, extended to
    /// corrupt content since a startup crash on a damaged state file would be
    /// worse than just losing the persisted runs.
    #[must_use]
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or_default()
    }

    /// # Errors
    /// Returns an error if the parent directory can't be created or the file
    /// can't be written.
    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(path, data)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_path() -> PathBuf {
        std::env::temp_dir().join(format!("lucid-daemon-state-test-{}", uuid::Uuid::new_v4()))
    }

    fn sample_run() -> WorkerRun {
        WorkerRun {
            issue_id: "ENG-9".into(),
            claim: ClaimState::Claimed(ClaimedSubstate::Running),
            phase: WorkerPhase::StreamingTurn,
            session_id: Some("session-1".into()),
            dispatch_id: Some("dispatch-1".into()),
            retries: 2,
            last_event_at: Utc::now(),
            last_error: None,
        }
    }

    #[test]
    fn missing_file_loads_as_empty_state() {
        let state = DaemonState::load(&scratch_path());
        assert!(state.runs.is_empty());
        assert!(state.last_pm_wake.is_none());
        assert!(state.last_mode.is_none());
    }

    #[test]
    fn corrupt_file_loads_as_empty_state() {
        let path = scratch_path();
        std::fs::write(&path, "not valid json").unwrap();
        let state = DaemonState::load(&path);
        assert!(state.runs.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn save_then_load_round_trips() {
        let path = scratch_path();
        let mut runs = HashMap::new();
        runs.insert("ENG-9".to_string(), sample_run());
        let state = DaemonState {
            runs,
            last_pm_wake: Some(Utc::now()),
            last_mode: Some(PresenceMode::Autonomous),
        };
        state.save(&path).unwrap();

        let loaded = DaemonState::load(&path);
        assert_eq!(loaded.runs.len(), 1);
        assert_eq!(loaded.runs["ENG-9"].issue_id, "ENG-9");
        assert_eq!(loaded.runs["ENG-9"].phase, WorkerPhase::StreamingTurn);
        assert_eq!(loaded.last_mode, Some(PresenceMode::Autonomous));
        assert!(loaded.last_pm_wake.is_some());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn terminal_phases_are_exhaustively_distinct_from_parked() {
        // A phase can't be both terminal and parked-for-human — if someone adds a
        // new phase later without deciding which bucket it belongs to, this either
        // stays trivially true or the two methods above need a matching update.
        let all = [
            WorkerPhase::PreparingWorkspace,
            WorkerPhase::BuildingPrompt,
            WorkerPhase::LaunchingAgentProcess,
            WorkerPhase::InitializingSession,
            WorkerPhase::StreamingTurn,
            WorkerPhase::AwaitingHumanInput,
            WorkerPhase::Finishing,
            WorkerPhase::Succeeded,
            WorkerPhase::Failed,
            WorkerPhase::TimedOut,
            WorkerPhase::Stalled,
            WorkerPhase::Stuck,
            WorkerPhase::CanceledByReconciliation,
        ];
        for phase in all {
            assert!(!(phase.is_terminal() && phase.is_parked_for_human()));
        }
    }
}
