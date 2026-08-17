//! Tracker adapter (see docs/wiki/architecture/tracker-adapter.md).
//!
//! Orchestrator logic talks to this trait only, never to a tracker-specific concept
//! directly — same "orchestration state separate from tracker state" principle as
//! Symphony. Linear is today's implementation; a second one (GitHub Issues) should
//! be possible without touching any caller of this trait.

pub mod file;
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

    /// Content-similarity search, for the death-loop-prevention dedup check (see
    /// docs/wiki/architecture/dedup-death-loop.md) — implementation decides what
    /// "similar" means (title match, content hash, etc.), the caller just needs
    /// "is there already something like this."
    async fn query_similar(&self, title: &str) -> anyhow::Result<Vec<TrackerIssue>>;

    /// Posts a proof-of-work artifact (a comment/note) onto an existing issue —
    /// e.g. the trace-query link a dispatch produces (see
    /// docs/wiki/architecture/trace-correlation.md). Never used to change decision
    /// state; that stays `set_decision_state`'s job.
    async fn attach_note(&self, issue_id: &str, body: &str) -> anyhow::Result<()>;
}

/// Builds the configured `TrackerAdapter` — the one place `backend` strings get
/// interpreted, so callers (the daemon, `pm::wake`, CLI handlers) never match on
/// the backend name themselves.
///
/// # Errors
/// Returns an error for an unrecognized `backend`, a `linear` backend missing
/// `team_key` or its API-key env var, or a `file` backend missing `file_path` /
/// failing to open its store.
pub fn build(config: &crate::config::TrackerConfig) -> anyhow::Result<Box<dyn TrackerAdapter>> {
    match config.backend.as_str() {
        "file" => {
            let path = config
                .file_path
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("tracker.file_path required for backend = \"file\""))?;
            Ok(Box::new(file::FileTracker::open(path)?))
        }
        "linear" => {
            let team_key = config
                .team_key
                .clone()
                .ok_or_else(|| anyhow::anyhow!("tracker.team_key required for backend = \"linear\""))?;
            let env_var = config
                .api_key_env
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("tracker.api_key_env required for backend = \"linear\""))?;
            let api_key = std::env::var(env_var)
                .map_err(|_| anyhow::anyhow!("env var `{env_var}` (tracker.api_key_env) is not set"))?;
            Ok(Box::new(linear::LinearAdapter::new(api_key, team_key)))
        }
        other => Err(anyhow::anyhow!(
            "unknown tracker backend `{other}` (expected \"file\" or \"linear\")"
        )),
    }
}
