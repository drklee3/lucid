//! Linear implementation of [`TrackerAdapter`], via Linear's own GraphQL API.
//!
//! Deliberately not routed through Linear's MCP server (see
//! docs/wiki/architecture/tracker-adapter.md: any MCP client library works, this
//! doesn't require Hermes's MCP wiring specifically) — a plain typed GraphQL client
//! is simpler and more robust for a non-agentic caller than going through a layer
//! built for LLM tool-calling.

use super::{DecisionState, EffortEstimate, Proposal, TrackerAdapter, TrackerIssue};
use anyhow::{Context, anyhow, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const API_URL: &str = "https://api.linear.app/graphql";

/// Linear caps page size server-side; 100 is under every documented cap.
const PAGE_SIZE: u32 = 100;

pub struct LinearAdapter {
    client: reqwest::Client,
    api_key: String,
    team_key: String,
}

impl LinearAdapter {
    /// `team_key` is Linear's short team key (e.g. `ENG`), not the team's UUID —
    /// it's what a human can read off the issue identifiers they're triaging.
    #[must_use]
    pub fn new(api_key: String, team_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            team_key,
        }
    }

    async fn graphql<T: DeserializeOwned>(
        &self,
        query: &str,
        variables: Value,
    ) -> anyhow::Result<T> {
        let response = self
            .client
            .post(API_URL)
            // Linear personal API keys go in a bare `Authorization` header — no `Bearer` prefix.
            .header("Authorization", &self.api_key)
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await
            .context("linear graphql request failed")?;

        let status = response.status();
        let body = response
            .text()
            .await
            .context("reading linear graphql response body")?;

        if !status.is_success() {
            bail!("linear graphql returned HTTP {status}: {body}");
        }

        let parsed: GraphQlResponse<T> = serde_json::from_str(&body)
            .with_context(|| format!("decoding linear graphql response: {body}"))?;

        if let Some(errors) = parsed.errors
            && !errors.is_empty()
        {
            let joined = errors
                .iter()
                .map(|e| e.message.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            bail!("linear graphql errors: {joined}");
        }

        parsed
            .data
            .ok_or_else(|| anyhow!("linear graphql response had neither data nor errors: {body}"))
    }

    /// Linear labels are addressed by UUID everywhere except this lookup, and the
    /// daemon never creates them — a missing label is a workspace-setup error, not
    /// something to paper over by silently mutating the workspace's label set.
    async fn label_id(&self, name: &str) -> anyhow::Result<String> {
        const QUERY: &str = r"
            query LabelId($name: String!, $team: String!) {
              issueLabels(filter: { name: { eq: $name }, team: { key: { eq: $team } } }, first: 1) {
                nodes { id }
              }
            }
        ";

        let data: LabelsData = self
            .graphql(QUERY, json!({ "name": name, "team": self.team_key }))
            .await?;

        data.issue_labels
            .nodes
            .into_iter()
            .next()
            .map(|node| node.id)
            .ok_or_else(|| {
                anyhow!(
                    "label `{name}` does not exist on team `{}` — create it in Linear first",
                    self.team_key
                )
            })
    }

    async fn issue_label_names(&self, issue_id: &str) -> anyhow::Result<Vec<LabelNode>> {
        const QUERY: &str = r"
            query IssueLabels($id: String!) {
              issue(id: $id) {
                labels(first: 100) { nodes { id name } }
              }
            }
        ";

        let data: IssueLabelsData = self.graphql(QUERY, json!({ "id": issue_id })).await?;
        let issue = data
            .issue
            .ok_or_else(|| anyhow!("linear issue `{issue_id}` not found"))?;
        Ok(issue.labels.nodes)
    }
}

#[async_trait]
impl TrackerAdapter for LinearAdapter {
    async fn create_proposal(&self, proposal: &Proposal) -> anyhow::Result<String> {
        const MUTATION: &str = r"
            mutation CreateProposal($input: IssueCreateInput!) {
              issueCreate(input: $input) {
                success
                issue { id }
              }
            }
        ";

        let label = self
            .label_id(decision_label(DecisionState::Pending))
            .await?;
        let team = team_id(
            &self
                .graphql(TEAM_ID_QUERY, json!({ "team": self.team_key }))
                .await?,
        )
        .ok_or_else(|| anyhow!("no Linear team with key `{}`", self.team_key))?;

        let input = json!({
            "teamId": team,
            "title": proposal.title,
            "description": render_description(proposal),
            "labelIds": [label],
        });

        let data: IssueCreateData = self.graphql(MUTATION, json!({ "input": input })).await?;
        let payload = data.issue_create;
        if !payload.success {
            bail!(
                "linear issueCreate reported failure for `{}`",
                proposal.title
            );
        }
        payload.issue.map(|issue| issue.id).ok_or_else(|| {
            anyhow!(
                "linear issueCreate returned no issue for `{}`",
                proposal.title
            )
        })
    }

    async fn set_decision_state(&self, issue_id: &str, state: DecisionState) -> anyhow::Result<()> {
        const ADD: &str = r"
            mutation AddLabel($id: String!, $labelId: String!) {
              issueAddLabel(id: $id, labelId: $labelId) { success }
            }
        ";
        const REMOVE: &str = r"
            mutation RemoveLabel($id: String!, $labelId: String!) {
              issueRemoveLabel(id: $id, labelId: $labelId) { success }
            }
        ";

        let target = decision_label(state);
        let current = self.issue_label_names(issue_id).await?;

        // Add/remove rather than `issueUpdate { labelIds }`: that input replaces the
        // whole set, which would drop labels a human added since this read.
        for stale in current
            .iter()
            .filter(|node| node.name.starts_with(LABEL_PREFIX) && node.name != target)
        {
            let data: RemoveLabelData = self
                .graphql(REMOVE, json!({ "id": issue_id, "labelId": stale.id }))
                .await?;
            if !data.issue_remove_label.success {
                bail!(
                    "linear issueRemoveLabel failed for `{}` on {issue_id}",
                    stale.name
                );
            }
        }

        if current.iter().any(|node| node.name == target) {
            return Ok(());
        }

        let target_id = self.label_id(target).await?;
        let data: AddLabelData = self
            .graphql(ADD, json!({ "id": issue_id, "labelId": target_id }))
            .await?;
        if !data.issue_add_label.success {
            bail!("linear issueAddLabel failed for `{target}` on {issue_id}");
        }
        Ok(())
    }

    async fn query_by_label(&self, label: &str) -> anyhow::Result<Vec<TrackerIssue>> {
        const QUERY: &str = r"
            query ByLabel($label: String!, $team: String!, $first: Int!, $after: String) {
              issues(
                filter: { labels: { some: { name: { eq: $label } } }, team: { key: { eq: $team } } }
                first: $first
                after: $after
              ) {
                nodes { id title labels(first: 100) { nodes { id name } } }
                pageInfo { hasNextPage endCursor }
              }
            }
        ";

        // Callers use this for the rejected-label dedup check, where a truncated page
        // reads as "no match" and files a duplicate — see
        // docs/wiki/architecture/dedup-death-loop.md. Paginate fully.
        let mut collected = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let data: IssuesData = self
                .graphql(
                    QUERY,
                    json!({
                        "label": label,
                        "team": self.team_key,
                        "first": PAGE_SIZE,
                        "after": cursor,
                    }),
                )
                .await?;

            collected.extend(
                data.issues
                    .nodes
                    .into_iter()
                    .map(IssueNode::into_tracker_issue),
            );

            if !data.issues.page_info.has_next_page {
                break;
            }
            cursor = data.issues.page_info.end_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(collected)
    }

    async fn query_similar(&self, title: &str) -> anyhow::Result<Vec<TrackerIssue>> {
        const QUERY: &str = r"
            query Similar($term: String!, $team: String!, $first: Int!) {
              searchIssues(
                term: $term
                filter: { team: { key: { eq: $team } } }
                includeArchived: true
                first: $first
              ) {
                nodes { id title labels(first: 100) { nodes { id name } } }
              }
            }
        ";

        // Linear's own full-text+vector search, rather than a client-side title
        // heuristic: a reworded duplicate is exactly what the dedup check must catch.
        // Rate-limited to 30/min server-side; failures surface rather than retry.
        let data: SearchData = self
            .graphql(
                QUERY,
                json!({ "term": title, "team": self.team_key, "first": 25 }),
            )
            .await?;

        Ok(data
            .search_issues
            .nodes
            .into_iter()
            .map(IssueNode::into_tracker_issue)
            .collect())
    }

    async fn attach_note(&self, issue_id: &str, body: &str) -> anyhow::Result<()> {
        const MUTATION: &str = r"
            mutation AttachNote($input: CommentCreateInput!) {
              commentCreate(input: $input) { success }
            }
        ";

        let data: CommentCreateData = self
            .graphql(
                MUTATION,
                json!({ "input": { "issueId": issue_id, "body": body } }),
            )
            .await?;
        if !data.comment_create.success {
            bail!("linear commentCreate failed on {issue_id}");
        }
        Ok(())
    }
}

const TEAM_ID_QUERY: &str = r"
    query TeamId($team: String!) {
      teams(filter: { key: { eq: $team } }, first: 1) { nodes { id } }
    }
";

const LABEL_PREFIX: &str = "proposal:";

fn team_id(data: &TeamsData) -> Option<String> {
    data.teams.nodes.first().map(|node| node.id.clone())
}

fn decision_label(state: DecisionState) -> &'static str {
    match state {
        DecisionState::Pending => "proposal:pending",
        DecisionState::Approved => "proposal:approved",
        DecisionState::Rejected => "proposal:rejected",
        DecisionState::StaleClosed => "proposal:stale",
    }
}

fn decision_from_label(name: &str) -> Option<DecisionState> {
    match name {
        "proposal:pending" => Some(DecisionState::Pending),
        "proposal:approved" => Some(DecisionState::Approved),
        "proposal:rejected" => Some(DecisionState::Rejected),
        "proposal:stale" => Some(DecisionState::StaleClosed),
        _ => None,
    }
}

fn effort_label(effort: EffortEstimate) -> &'static str {
    match effort {
        EffortEstimate::Small => "S",
        EffortEstimate::Medium => "M",
        EffortEstimate::Large => "L",
    }
}

/// The Worker parses the frontmatter block deterministically (see
/// docs/wiki/architecture/agent-handoff.md), so every scalar and list is emitted as
/// JSON — a YAML 1.2 subset — instead of bare text that quoting could break.
fn render_description(proposal: &Proposal) -> String {
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

fn yaml_scalar(value: &str) -> String {
    Value::String(value.to_string()).to_string()
}

fn yaml_list(values: &[String]) -> String {
    Value::from(values.to_vec()).to_string()
}

#[derive(Deserialize)]
struct GraphQlResponse<T> {
    data: Option<T>,
    errors: Option<Vec<GraphQlError>>,
}

#[derive(Deserialize)]
struct GraphQlError {
    message: String,
}

#[derive(Deserialize)]
struct LabelNode {
    id: String,
    name: String,
}

#[derive(Deserialize)]
struct Nodes<T> {
    nodes: Vec<T>,
}

#[derive(Deserialize)]
struct IdNode {
    id: String,
}

#[derive(Deserialize)]
struct IssueNode {
    id: String,
    title: String,
    labels: Nodes<LabelNode>,
}

impl IssueNode {
    fn into_tracker_issue(self) -> TrackerIssue {
        let decision_state = self
            .labels
            .nodes
            .iter()
            .find_map(|node| decision_from_label(&node.name));
        TrackerIssue {
            id: self.id,
            title: self.title,
            decision_state,
        }
    }
}

#[derive(Deserialize)]
struct PageInfo {
    #[serde(rename = "hasNextPage")]
    has_next_page: bool,
    #[serde(rename = "endCursor")]
    end_cursor: Option<String>,
}

#[derive(Deserialize)]
struct IssueConnection {
    nodes: Vec<IssueNode>,
    #[serde(rename = "pageInfo")]
    page_info: PageInfo,
}

#[derive(Deserialize)]
struct TeamsData {
    teams: Nodes<IdNode>,
}

#[derive(Deserialize)]
struct LabelsData {
    #[serde(rename = "issueLabels")]
    issue_labels: Nodes<IdNode>,
}

#[derive(Deserialize)]
struct IssueLabelsData {
    issue: Option<IssueWithLabels>,
}

#[derive(Deserialize)]
struct IssueWithLabels {
    labels: Nodes<LabelNode>,
}

#[derive(Deserialize)]
struct IssuesData {
    issues: IssueConnection,
}

#[derive(Deserialize)]
struct SearchData {
    #[serde(rename = "searchIssues")]
    search_issues: Nodes<IssueNode>,
}

#[derive(Deserialize)]
struct IssueCreateData {
    #[serde(rename = "issueCreate")]
    issue_create: IssuePayload,
}

#[derive(Deserialize)]
struct IssuePayload {
    success: bool,
    issue: Option<IdNode>,
}

#[derive(Deserialize)]
struct SuccessPayload {
    success: bool,
}

#[derive(Deserialize)]
struct AddLabelData {
    #[serde(rename = "issueAddLabel")]
    issue_add_label: SuccessPayload,
}

#[derive(Deserialize)]
struct RemoveLabelData {
    #[serde(rename = "issueRemoveLabel")]
    issue_remove_label: SuccessPayload,
}

#[derive(Deserialize)]
struct CommentCreateData {
    #[serde(rename = "commentCreate")]
    comment_create: SuccessPayload,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Proposal {
        Proposal {
            title: "Fix presence detection".to_string(),
            summary: "IdleHint never resets.".to_string(),
            why_now: vec!["Blocks autonomous mode".to_string()],
            effort_estimate: EffortEstimate::Medium,
            risk_note: "Low: read-only probe".to_string(),
            task_type: "bug: fix".to_string(),
            target_paths: vec!["src/presence/mod.rs".to_string()],
            acceptance_criteria: vec!["Idle flips after threshold".to_string()],
            research_ref: None,
        }
    }

    #[test]
    fn description_frontmatter_is_parseable_json_per_field() {
        let rendered = render_description(&sample());
        let frontmatter = rendered
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .expect("frontmatter block delimited");

        let mut fields = std::collections::HashMap::new();
        for line in frontmatter.0.lines() {
            let (key, value) = line.split_once(": ").expect("key: value line");
            fields.insert(
                key,
                serde_json::from_str::<Value>(value).expect("json value"),
            );
        }

        assert_eq!(fields["task_type"], json!("bug: fix"));
        assert_eq!(fields["target_paths"], json!(["src/presence/mod.rs"]));
        assert_eq!(
            fields["acceptance_criteria"],
            json!(["Idle flips after threshold"])
        );
        assert_eq!(fields["research_ref"], Value::Null);
    }

    #[test]
    fn description_body_carries_effort_and_reasons() {
        let rendered = render_description(&sample());
        assert!(rendered.contains("- Blocks autonomous mode"));
        assert!(rendered.contains("**Effort:** M"));
        assert!(rendered.contains("**Risk:** Low: read-only probe"));
    }

    #[test]
    fn decision_labels_round_trip() {
        for state in [
            DecisionState::Pending,
            DecisionState::Approved,
            DecisionState::Rejected,
            DecisionState::StaleClosed,
        ] {
            let label = decision_label(state);
            assert!(label.starts_with(LABEL_PREFIX));
            assert_eq!(decision_from_label(label), Some(state));
        }
        assert_eq!(decision_from_label("area:presence"), None);
    }

    #[test]
    fn issue_node_reads_decision_state_from_labels() {
        let node: IssueNode = serde_json::from_value(json!({
            "id": "abc",
            "title": "Fix presence detection",
            "labels": { "nodes": [
                { "id": "l1", "name": "area:presence" },
                { "id": "l2", "name": "proposal:approved" }
            ] }
        }))
        .expect("issue node");

        let issue = node.into_tracker_issue();
        assert_eq!(issue.id, "abc");
        assert_eq!(issue.decision_state, Some(DecisionState::Approved));
    }

    #[test]
    fn graphql_errors_take_precedence_over_data() {
        let response: GraphQlResponse<TeamsData> = serde_json::from_value(json!({
            "data": null,
            "errors": [{ "message": "Authentication required" }]
        }))
        .expect("error envelope");

        let messages: Vec<_> = response
            .errors
            .expect("errors present")
            .into_iter()
            .map(|e| e.message)
            .collect();
        assert_eq!(messages, ["Authentication required"]);
        assert!(response.data.is_none());
    }

    #[test]
    fn teams_response_yields_team_id() {
        let data: TeamsData =
            serde_json::from_value(json!({ "teams": { "nodes": [{ "id": "team-uuid" }] } }))
                .expect("teams data");
        assert_eq!(team_id(&data).as_deref(), Some("team-uuid"));

        let empty: TeamsData =
            serde_json::from_value(json!({ "teams": { "nodes": [] } })).expect("teams data");
        assert_eq!(team_id(&empty), None);
    }
}
