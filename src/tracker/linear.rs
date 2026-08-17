//! Linear implementation of [`TrackerAdapter`], via Linear's own GraphQL API.
//!
//! Deliberately not routed through Linear's MCP server (design.md: "any MCP client
//! library works, this doesn't require Hermes's MCP wiring specifically") — a plain
//! typed GraphQL client is simpler and more robust for a non-agentic caller than
//! going through a layer built for LLM tool-calling.

use super::{DecisionState, Proposal, TrackerAdapter, TrackerIssue};
use async_trait::async_trait;

pub struct LinearAdapter {
    client: reqwest::Client,
    api_key: String,
}

impl LinearAdapter {
    pub fn new(api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
        }
    }
}

#[async_trait]
impl TrackerAdapter for LinearAdapter {
    async fn create_proposal(&self, proposal: &Proposal) -> anyhow::Result<String> {
        let _ = (&self.client, &self.api_key, proposal);
        todo!("issueCreate mutation against api.linear.app/graphql")
    }

    async fn set_decision_state(
        &self,
        issue_id: &str,
        state: DecisionState,
    ) -> anyhow::Result<()> {
        let _ = (issue_id, state);
        todo!("issueUpdate mutation — set proposal:pending/approved/rejected label")
    }

    async fn query_by_label(&self, label: &str) -> anyhow::Result<Vec<TrackerIssue>> {
        let _ = label;
        todo!("issues(filter: {{ labels: {{ name: {{ eq: $label }} }} }}) query")
    }

    async fn query_similar(&self, title: &str) -> anyhow::Result<Vec<TrackerIssue>> {
        let _ = title;
        todo!("title/content similarity query for dedup (decision #6)")
    }
}
