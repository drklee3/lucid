//! File-backed `TrackerAdapter` — a local JSON store standing in for a real
//! tracker backend while `LinearAdapter`'s GraphQL calls are still unimplemented.
//! Not meant to survive past that: it exists so the dispatch → trace-link → note
//! loop (see docs/wiki/architecture/trace-correlation.md) can be proven end-to-end
//! today without needing Linear API credentials.

use super::{
    DecisionState, Proposal, ReviewMode, TrackerAdapter, TrackerIssue, render_comment,
    render_description,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredIssue {
    id: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    review: ReviewMode,
    decision_state: Option<DecisionState>,
    notes: Vec<String>,
}

pub struct FileTracker {
    path: PathBuf,
    issues: Mutex<Vec<StoredIssue>>,
}

impl FileTracker {
    /// Opens (or creates) the JSON store at `path`. Reading and writing happen
    /// synchronously on the calling task — the store is small (a handful of local
    /// issues) and low-frequency, so this doesn't warrant `spawn_blocking`.
    ///
    /// # Errors
    /// Returns an error if an existing file can't be read or doesn't parse as the
    /// expected JSON shape.
    pub fn open(path: impl Into<PathBuf>) -> anyhow::Result<Self> {
        let path = path.into();
        let issues = if path.exists() {
            let data = std::fs::read_to_string(&path)?;
            serde_json::from_str(&data)?
        } else {
            Vec::new()
        };
        Ok(Self {
            path,
            issues: Mutex::new(issues),
        })
    }

    fn persist(&self, issues: &[StoredIssue]) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_string_pretty(issues)?)?;
        Ok(())
    }

    fn as_tracker_issue(issue: &StoredIssue) -> TrackerIssue {
        TrackerIssue {
            id: issue.id.clone(),
            title: issue.title.clone(),
            description: issue.description.clone(),
            decision_state: issue.decision_state,
            review: issue.review,
        }
    }
}

#[async_trait]
impl TrackerAdapter for FileTracker {
    async fn create_proposal(&self, proposal: &Proposal) -> anyhow::Result<String> {
        let mut issues = self.issues.lock().unwrap();
        let id = format!("LOCAL-{}", issues.len() + 1);
        issues.push(StoredIssue {
            id: id.clone(),
            title: proposal.title.clone(),
            description: Some(render_description(proposal)),
            review: proposal.review,
            decision_state: Some(DecisionState::Pending),
            notes: Vec::new(),
        });
        self.persist(&issues)?;
        Ok(id)
    }

    async fn set_decision_state(&self, issue_id: &str, state: DecisionState) -> anyhow::Result<()> {
        let mut issues = self.issues.lock().unwrap();
        let issue = issues
            .iter_mut()
            .find(|i| i.id == issue_id)
            .ok_or_else(|| anyhow::anyhow!("no such issue: {issue_id}"))?;
        issue.decision_state = Some(state);
        self.persist(&issues)
    }

    async fn query_by_decision_state(
        &self,
        state: DecisionState,
    ) -> anyhow::Result<Vec<TrackerIssue>> {
        let issues = self.issues.lock().unwrap();
        Ok(issues
            .iter()
            .filter(|i| i.decision_state == Some(state))
            .map(Self::as_tracker_issue)
            .collect())
    }

    async fn query_similar(&self, title: &str) -> anyhow::Result<Vec<TrackerIssue>> {
        let needle = title.to_lowercase();
        let issues = self.issues.lock().unwrap();
        Ok(issues
            .iter()
            .filter(|i| i.title.to_lowercase().contains(&needle))
            .map(Self::as_tracker_issue)
            .collect())
    }

    /// Auto-creates a bare placeholder issue if `issue_id` isn't already tracked —
    /// this lets the dispatch loop be exercised against an arbitrary id without
    /// requiring `create_proposal` to have run first (useful for a standalone e2e
    /// smoke test; a real tracker backend wouldn't do this).
    async fn attach_note(&self, issue_id: &str, body: &str) -> anyhow::Result<()> {
        let mut issues = self.issues.lock().unwrap();
        if let Some(issue) = issues.iter_mut().find(|i| i.id == issue_id) {
            issue.notes.push(body.to_string());
        } else {
            issues.push(StoredIssue {
                id: issue_id.to_string(),
                title: format!("(auto-created by attach_note for {issue_id})"),
                description: None,
                review: ReviewMode::Auto,
                decision_state: None,
                notes: vec![body.to_string()],
            });
        }
        self.persist(&issues)
    }

    async fn list_comments(&self, issue_id: &str) -> anyhow::Result<Vec<String>> {
        let issues = self.issues.lock().unwrap();
        Ok(issues
            .iter()
            .find(|i| i.id == issue_id)
            .map(|issue| {
                issue
                    .notes
                    .iter()
                    .map(|note| render_comment("lucid", note))
                    .collect()
            })
            .unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::{EffortEstimate, ReviewMode};

    fn proposal(title: &str) -> Proposal {
        Proposal {
            title: title.to_string(),
            summary: "summary".to_string(),
            why_now: vec!["because".to_string()],
            effort_estimate: EffortEstimate::Small,
            risk_note: "none".to_string(),
            task_type: "feature".to_string(),
            target_paths: vec![],
            acceptance_criteria: vec![],
            research_ref: None,
            review: ReviewMode::Auto,
            verify_cmd: None,
        }
    }

    fn scratch_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "lucid-file-tracker-test-{name}-{}.json",
            uuid::Uuid::new_v4()
        ))
    }

    #[tokio::test]
    async fn create_then_query_by_decision_state_and_similar() {
        let path = scratch_path("create-query");
        let tracker = FileTracker::open(&path).unwrap();

        let id = tracker
            .create_proposal(&proposal("Fix the flaky presence test"))
            .await
            .unwrap();

        let pending = tracker
            .query_by_decision_state(DecisionState::Pending)
            .await
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, id);

        let similar = tracker.query_similar("flaky presence").await.unwrap();
        assert_eq!(similar.len(), 1);

        let none = tracker.query_similar("something unrelated").await.unwrap();
        assert!(none.is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn set_decision_state_persists_across_reopen() {
        let path = scratch_path("persist");
        let tracker = FileTracker::open(&path).unwrap();
        let id = tracker
            .create_proposal(&proposal("Some proposal"))
            .await
            .unwrap();
        tracker
            .set_decision_state(&id, DecisionState::Approved)
            .await
            .unwrap();

        let reopened = FileTracker::open(&path).unwrap();
        // The `Pending` state from create_proposal must have been replaced, not
        // just left alongside the new one — otherwise the issue would show up
        // under both queries.
        assert!(
            reopened
                .query_by_decision_state(DecisionState::Pending)
                .await
                .unwrap()
                .is_empty()
        );
        let found = reopened
            .query_by_decision_state(DecisionState::Approved)
            .await
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].decision_state, Some(DecisionState::Approved));

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn approving_makes_the_issue_visible_to_the_dispatch_loop() {
        let path = scratch_path("approve-visibility");
        let tracker = FileTracker::open(&path).unwrap();
        let id = tracker
            .create_proposal(&proposal("Ship the thing"))
            .await
            .unwrap();

        assert!(
            tracker
                .query_by_decision_state(DecisionState::Approved)
                .await
                .unwrap()
                .is_empty()
        );
        tracker
            .set_decision_state(&id, DecisionState::Approved)
            .await
            .unwrap();
        assert_eq!(
            tracker
                .query_by_decision_state(DecisionState::Approved)
                .await
                .unwrap()
                .len(),
            1
        );

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn attach_note_auto_creates_an_unknown_issue() {
        let path = scratch_path("auto-create");
        let tracker = FileTracker::open(&path).unwrap();
        tracker.attach_note("ENG-999", "hello").await.unwrap();

        let reopened = FileTracker::open(&path).unwrap();
        let issues = reopened.issues.lock().unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].id, "ENG-999");
        assert_eq!(issues[0].notes, vec!["hello".to_string()]);

        drop(issues);
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn list_comments_returns_notes_in_order() {
        let path = scratch_path("list-comments");
        let tracker = FileTracker::open(&path).unwrap();
        let id = tracker
            .create_proposal(&proposal("Some proposal"))
            .await
            .unwrap();

        tracker.attach_note(&id, "first note").await.unwrap();
        tracker.attach_note(&id, "second note").await.unwrap();

        let comments = tracker.list_comments(&id).await.unwrap();
        assert_eq!(
            comments,
            vec![
                "lucid: first note".to_string(),
                "lucid: second note".to_string()
            ]
        );

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn list_comments_is_empty_for_an_unknown_issue() {
        let path = scratch_path("list-comments-unknown");
        let tracker = FileTracker::open(&path).unwrap();
        assert!(tracker.list_comments("NOPE-1").await.unwrap().is_empty());
        let _ = std::fs::remove_file(&path);
    }
}
