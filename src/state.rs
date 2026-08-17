//! Worker and orchestration state machines.
//!
//! Exhaustive `match` on these enums is the whole point of choosing Rust for this
//! project (see docs/design.md, "Implementation language: Rust") — adding a state
//! and forgetting to handle it somewhere becomes a compile error, not a live bug
//! discovered the way OpenHands', cyrus's, and Symphony's own state gaps were.

use chrono::{DateTime, Utc};

/// Orchestration state, separate from whatever the tracker calls its own statuses
/// (Symphony's pattern — design.md, "Symphony SPEC.md" section).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimState {
    Unclaimed,
    Claimed(ClaimedSubstate),
    Released,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimedSubstate {
    Running,
    RetryQueued,
}

/// Per-run Worker phase. Symphony's eleven phases, plus two states nothing surveyed
/// had (design.md, "UX / State-Machine Gap Analysis" — State machine section):
///
/// - `AwaitingHumanInput`: the Worker paused mid-task to ask a clarifying question
///   (Linear Agent Sessions' `awaitingInput`/`elicitation` pattern — we had no
///   equivalent before this).
/// - `Stuck`: detected unproductive looping (busywork, not silence) — distinct from
///   `Stalled`, which is purely `elapsed_ms > stall_timeout_ms`. OpenHands has this
///   as a first-class state; Symphony does not. The *detection logic* for this is
///   explicitly not designed yet (docs/FEATURES.md, Deferred) — only the state
///   exists so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            WorkerPhase::Succeeded
                | WorkerPhase::Failed
                | WorkerPhase::TimedOut
                | WorkerPhase::CanceledByReconciliation
        )
    }

    /// Symphony's "parked state stops polling" rule (design.md gap analysis),
    /// made explicit here rather than left implicit: once a Worker needs a human,
    /// the reconciliation tick should stop actively dispatching/polling it.
    pub fn is_parked_for_human(self) -> bool {
        matches!(self, WorkerPhase::AwaitingHumanInput)
    }
}

/// One tracked run of a Worker against a single tracker item.
#[derive(Debug, Clone)]
pub struct WorkerRun {
    pub issue_id: String,
    pub claim: ClaimState,
    pub phase: WorkerPhase,
    pub session_id: Option<String>,
    pub retries: u32,
    pub last_event_at: DateTime<Utc>,
    pub last_error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
