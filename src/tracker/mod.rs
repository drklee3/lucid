//! Tracker adapter (docs/design.md, resolved decision #3).
//!
//! Orchestrator logic talks to this trait only, never to a tracker-specific concept
//! directly — same "orchestration state separate from tracker state" principle as
//! Symphony. Linear is today's implementation; a second one (GitHub Issues) should
//! be possible without touching any caller of this trait.

pub mod linear;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub title: String,
    pub summary: String,
    pub why_now: Vec<String>,
    pub effort_estimate: EffortEstimate,
    pub risk_note: String,
    pub task_type: String,
    pub target_paths: Vec<String>,
    pub acceptance_criteria: Vec<String>,
    pub research_ref: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum EffortEstimate {
    Small,
    Medium,
    Large,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DecisionState {
    Pending,
    Approved,
    Rejected,
    StaleClosed,
}

#[derive(Debug, Clone)]
pub struct TrackerIssue {
    pub id: String,
    pub title: String,
    pub decision_state: Option<DecisionState>,
}

#[async_trait::async_trait]
pub trait TrackerAdapter: Send + Sync {
    /// File a new proposal (a PM gap-flag stub). Returns the created issue's id.
    async fn create_proposal(&self, proposal: &Proposal) -> anyhow::Result<String>;

    /// Move an existing issue's decision state (approve/reject/stale-close).
    async fn set_decision_state(
        &self,
        issue_id: &str,
        state: DecisionState,
    ) -> anyhow::Result<()>;

    /// Issues carrying a given label — used for dedup (rejected-label check) and
    /// for finding open work in a given state.
    async fn query_by_label(&self, label: &str) -> anyhow::Result<Vec<TrackerIssue>>;

    /// Content-similarity search, for the death-loop-prevention dedup check
    /// (decision #6) — implementation decides what "similar" means (title match,
    /// content hash, etc.), the caller just needs "is there already something like
    /// this."
    async fn query_similar(&self, title: &str) -> anyhow::Result<Vec<TrackerIssue>>;
}
