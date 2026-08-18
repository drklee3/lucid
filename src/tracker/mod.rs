//! Tracker adapter (see docs/wiki/architecture/tracker-adapter.md).
//!
//! Orchestrator logic talks to this trait only, never to a tracker-specific concept
//! directly — same "orchestration state separate from tracker state" principle as
//! Symphony. Linear is today's implementation; a second one (GitHub Issues) should
//! be possible without touching any caller of this trait.

pub mod file;
pub mod linear;

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    /// Who gets to close this one out — see docs/wiki/architecture/worker-completion.md.
    #[serde(default)]
    pub review: ReviewMode,
    /// Shell command a `ReviewMode::Agent` review runs in `daemon.workdir` before
    /// judging the diff — lucid runs it and checks the exit code itself,
    /// deterministically, rather than trusting an LLM's read of whether it passed.
    /// `None` lets the review agent infer its own command from the repo's
    /// conventions (`Cargo.toml`, `package.json`, `CLAUDE.md`, ...) instead of
    /// requiring this to be set for every proposal. See
    /// docs/wiki/architecture/worker-completion.md.
    #[serde(default)]
    pub verify_cmd: Option<String>,
}

/// How a successful dispatch's outcome gets finalized — the fork
/// docs/wiki/architecture/review-rework-ux.md left open, resolved per-issue rather
/// than per-repo so a human can dial trust up or down ticket by ticket.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReviewMode {
    /// A harness-reported success moves the issue straight to `Done` — no gate.
    #[default]
    Auto,
    /// A harness-reported success moves the issue to `NeedsReview` and stops —
    /// a human has to look and flip it themselves.
    Human,
    /// A harness-reported success triggers a second, read-only dispatch that
    /// reviews the diff against `acceptance_criteria`; its verdict decides
    /// `Done` vs `NeedsReview`.
    Agent,
}

pub const REVIEW_LABEL_PREFIX: &str = "review:";

#[must_use]
pub fn review_label(mode: ReviewMode) -> &'static str {
    match mode {
        ReviewMode::Auto => "review:auto",
        ReviewMode::Human => "review:human",
        ReviewMode::Agent => "review:agent",
    }
}

#[must_use]
pub fn review_from_label(name: &str) -> Option<ReviewMode> {
    match name {
        "review:auto" => Some(ReviewMode::Auto),
        "review:human" => Some(ReviewMode::Human),
        "review:agent" => Some(ReviewMode::Agent),
        _ => None,
    }
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
    /// A dispatch for this issue ran and the harness reported success — set by
    /// the daemon itself, not a human (see docs/wiki/architecture/worker-completion.md).
    /// Distinct from `Approved`, which just means "safe to dispatch."
    Done,
    /// A dispatch succeeded but `ReviewMode::Human`/`Agent` didn't clear it —
    /// parked until a human moves it (see docs/wiki/architecture/worker-completion.md).
    /// The dispatch loop never re-picks this up on its own.
    NeedsReview,
}

pub const LABEL_PREFIX: &str = "proposal:";

#[must_use]
pub fn decision_label(state: DecisionState) -> &'static str {
    match state {
        DecisionState::Pending => "proposal:pending",
        DecisionState::Approved => "proposal:approved",
        DecisionState::Rejected => "proposal:rejected",
        DecisionState::StaleClosed => "proposal:stale",
        DecisionState::Done => "proposal:done",
        DecisionState::NeedsReview => "proposal:needs-review",
    }
}

#[must_use]
pub fn decision_from_label(name: &str) -> Option<DecisionState> {
    match name {
        "proposal:pending" => Some(DecisionState::Pending),
        "proposal:approved" => Some(DecisionState::Approved),
        "proposal:rejected" => Some(DecisionState::Rejected),
        "proposal:stale" => Some(DecisionState::StaleClosed),
        "proposal:done" => Some(DecisionState::Done),
        "proposal:needs-review" => Some(DecisionState::NeedsReview),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct TrackerIssue {
    pub id: String,
    pub title: String,
    /// The frontmatter+body handoff surface a Worker parses deterministically —
    /// see docs/wiki/architecture/agent-handoff.md. `None` for issues created
    /// outside `create_proposal` (e.g. hand-filed tracker items).
    pub description: Option<String>,
    pub decision_state: Option<DecisionState>,
    /// Defaults to `ReviewMode::Auto` for issues with no `review:*` label/field —
    /// matches today's behavior (a successful dispatch just... finishes) for
    /// every issue created before this field existed.
    pub review: ReviewMode,
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

fn effort_label(effort: EffortEstimate) -> &'static str {
    match effort {
        EffortEstimate::Small => "S",
        EffortEstimate::Medium => "M",
        EffortEstimate::Large => "L",
    }
}

/// Renders a `Proposal` into the frontmatter+body handoff surface a Worker parses
/// deterministically (see docs/wiki/architecture/agent-handoff.md) — shared by every
/// `TrackerAdapter` implementation so the surface a Worker sees doesn't drift by
/// backend. Every scalar and list is emitted as JSON — a YAML 1.2 subset — instead
/// of bare text that quoting could break.
#[must_use]
pub fn render_description(proposal: &Proposal) -> String {
    use std::fmt::Write;

    let research_ref = proposal
        .research_ref
        .as_deref()
        .map_or_else(|| "null".to_string(), yaml_scalar);

    let mut out = String::from("---\n");
    let _ = writeln!(out, "task_type: {}", yaml_scalar(&proposal.task_type));
    let _ = writeln!(out, "target_paths: {}", yaml_list(&proposal.target_paths));
    let _ = writeln!(
        out,
        "acceptance_criteria: {}",
        yaml_list(&proposal.acceptance_criteria)
    );
    let _ = writeln!(out, "research_ref: {research_ref}");
    let _ = writeln!(out, "review: {}", yaml_scalar(review_label(proposal.review)));
    let verify_cmd = proposal
        .verify_cmd
        .as_deref()
        .map_or_else(|| "null".to_string(), yaml_scalar);
    let _ = writeln!(out, "verify_cmd: {verify_cmd}");
    out.push_str("---\n\n");

    out.push_str(&proposal.summary);
    out.push_str("\n\n## Why now\n\n");
    for reason in &proposal.why_now {
        let _ = writeln!(out, "- {reason}");
    }
    let _ = write!(
        out,
        "\n**Effort:** {}\n\n**Risk:** {}\n",
        effort_label(proposal.effort_estimate),
        proposal.risk_note
    );
    out
}

/// Reads one scalar string field back out of a `render_description`-rendered
/// frontmatter block — the same JSON-per-field format
/// `description_frontmatter_is_parseable_json_per_field` already covers.
///
/// This is a deliberate, narrow exception to the rule the rest of the frontmatter
/// follows (see docs/wiki/architecture/agent-handoff.md): most fields are meant
/// for the *harness* to read as prose, not for lucid's own code to parse back out
/// — `review` gets that treatment via a dedicated label instead. `verify_cmd` is
/// free-text (a shell command), which can't be encoded as a label the way a small
/// enum can, so it goes through this parser instead. Returns `None` if there's no
/// frontmatter, the key is absent, or its value is JSON `null`.
#[must_use]
pub fn frontmatter_field(description: Option<&str>, key: &str) -> Option<String> {
    let frontmatter = description?.strip_prefix("---\n")?.split_once("\n---\n")?.0;
    for line in frontmatter.lines() {
        if let Some((k, v)) = line.split_once(": ") {
            if k == key {
                return match serde_json::from_str::<Value>(v).ok()? {
                    Value::String(s) => Some(s),
                    _ => None,
                };
            }
        }
    }
    None
}

fn yaml_scalar(value: &str) -> String {
    Value::String(value.to_string()).to_string()
}

fn yaml_list(values: &[String]) -> String {
    Value::from(values.to_vec()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_proposal(verify_cmd: Option<&str>) -> Proposal {
        Proposal {
            title: "t".into(),
            summary: "s".into(),
            why_now: vec![],
            effort_estimate: EffortEstimate::Small,
            risk_note: "r".into(),
            task_type: "chore".into(),
            target_paths: vec![],
            acceptance_criteria: vec![],
            research_ref: None,
            review: ReviewMode::Auto,
            verify_cmd: verify_cmd.map(str::to_string),
        }
    }

    #[test]
    fn frontmatter_field_reads_a_set_value() {
        let description = render_description(&sample_proposal(Some("cargo test")));
        assert_eq!(
            frontmatter_field(Some(&description), "verify_cmd"),
            Some("cargo test".to_string())
        );
    }

    #[test]
    fn frontmatter_field_is_none_for_a_null_value() {
        let description = render_description(&sample_proposal(None));
        assert_eq!(frontmatter_field(Some(&description), "verify_cmd"), None);
    }

    #[test]
    fn frontmatter_field_is_none_for_an_absent_key() {
        let description = render_description(&sample_proposal(None));
        assert_eq!(frontmatter_field(Some(&description), "no_such_key"), None);
    }

    #[test]
    fn frontmatter_field_is_none_without_a_description() {
        assert_eq!(frontmatter_field(None, "verify_cmd"), None);
    }
}
