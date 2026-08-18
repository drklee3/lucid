//! Linear implementation of [`TrackerAdapter`], via Linear's own GraphQL API.
//!
//! Deliberately not routed through Linear's MCP server (see
//! docs/wiki/architecture/tracker-adapter.md: any MCP client library works, this
//! doesn't require Hermes's MCP wiring specifically) — a plain typed GraphQL client
//! is simpler and more robust for a non-agentic caller than going through a layer
//! built for LLM tool-calling.

use super::{
    DecisionState, Proposal, ReviewMode, TrackerAdapter, TrackerIssue, decision_state_from_name,
    decision_state_name, render_description, review_from_label, review_label,
};
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
    project_key: Option<String>,
}

impl LinearAdapter {
    /// `team_key` is Linear's short team key (e.g. `ENG`), not the team's UUID —
    /// it's what a human can read off the issue identifiers they're triaging.
    /// `project_key` optionally scopes lucid to a single project within that team
    /// (Linear issues don't require a project, so `None` leaves it team-wide).
    #[must_use]
    pub fn new(api_key: String, team_key: String, project_key: Option<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
            team_key,
            project_key,
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

    /// Team scope, plus project scope when `project_key` is configured. Built as a
    /// JSON value (rather than a `$project` GraphQL variable) so an unset project
    /// omits the clause entirely instead of filtering for a literal null project.
    fn team_and_project_filter(&self) -> Value {
        let mut filter = json!({ "team": { "key": { "eq": self.team_key } } });
        if let Some(project) = &self.project_key {
            filter["project"] = json!({ "name": { "eq": project } });
        }
        filter
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

    /// Resolves `project_key` (a project name) to its id — only called when
    /// `project_key` is configured. Like `label_id`/`state_id`, lucid never
    /// creates projects: a missing one is a workspace-setup error.
    async fn project_id(&self, name: &str) -> anyhow::Result<String> {
        const QUERY: &str = r"
            query ProjectId($name: String!) {
              projects(filter: { name: { eq: $name } }, first: 1) {
                nodes { id }
              }
            }
        ";

        let data: ProjectsData = self.graphql(QUERY, json!({ "name": name })).await?;

        data.projects
            .nodes
            .into_iter()
            .next()
            .map(|node| node.id)
            .ok_or_else(|| anyhow!("project `{name}` does not exist — create it in Linear first"))
    }

    /// Decision state moves the issue's real `state` field, not a label — see
    /// docs/wiki/architecture/tracker-adapter.md. Like `label_id`, lucid never
    /// creates these: a missing one is a workspace-setup error.
    async fn state_id(&self, name: &str) -> anyhow::Result<String> {
        const QUERY: &str = r"
            query StateId($name: String!, $team: String!) {
              workflowStates(filter: { name: { eq: $name }, team: { key: { eq: $team } } }, first: 1) {
                nodes { id }
              }
            }
        ";

        let data: WorkflowStatesData = self
            .graphql(QUERY, json!({ "name": name, "team": self.team_key }))
            .await?;

        data.workflow_states
            .nodes
            .into_iter()
            .next()
            .map(|node| node.id)
            .ok_or_else(|| {
                anyhow!(
                    "workflow state `{name}` does not exist on team `{}` — create it in Linear first",
                    self.team_key
                )
            })
    }

    /// Every archived issue in the team — the closest read-back Linear offers
    /// for `Rejected`/`StaleClosed`, which share the archive mechanism instead
    /// of a distinct workflow state. See `decision_state_name`.
    async fn query_archived(&self) -> anyhow::Result<Vec<TrackerIssue>> {
        const QUERY: &str = r"
            query Archived($filter: IssueFilter!, $first: Int!, $after: String) {
              issues(
                filter: $filter
                includeArchived: true
                first: $first
                after: $after
              ) {
                nodes { id title description state { name } archivedAt labels(first: 100) { nodes { id name } } }
                pageInfo { hasNextPage endCursor }
              }
            }
        ";

        let mut collected = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let data: IssuesData = self
                .graphql(
                    QUERY,
                    json!({ "filter": self.team_and_project_filter(), "first": PAGE_SIZE, "after": cursor }),
                )
                .await?;

            collected.extend(
                data.issues
                    .nodes
                    .into_iter()
                    .map(IssueNode::into_tracker_issue)
                    .filter(|issue| issue.decision_state == Some(DecisionState::Rejected)),
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

        let pending_state_id = self
            .state_id(
                decision_state_name(DecisionState::Pending)
                    .expect("Pending is always a real state"),
            )
            .await?;
        let review_label_id = self.label_id(review_label(proposal.review)).await?;
        let team = team_id(
            &self
                .graphql(TEAM_ID_QUERY, json!({ "team": self.team_key }))
                .await?,
        )
        .ok_or_else(|| anyhow!("no Linear team with key `{}`", self.team_key))?;

        let mut input = json!({
            "teamId": team,
            "title": proposal.title,
            "description": render_description(proposal),
            "stateId": pending_state_id,
            "labelIds": [review_label_id],
        });
        if let Some(project) = &self.project_key {
            input["projectId"] = json!(self.project_id(project).await?);
        }

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
        const UPDATE: &str = r"
            mutation SetState($id: String!, $stateId: String!) {
              issueUpdate(id: $id, input: { stateId: $stateId }) { success }
            }
        ";
        const ARCHIVE: &str = r"
            mutation ArchiveIssue($id: String!) {
              issueArchive(id: $id) { success }
            }
        ";

        // `Rejected`/`StaleClosed` archive rather than move to a state — see
        // `decision_state_name`'s doc comment.
        let Some(name) = decision_state_name(state) else {
            let data: IssueArchiveData = self.graphql(ARCHIVE, json!({ "id": issue_id })).await?;
            if !data.issue_archive.success {
                bail!("linear issueArchive failed on {issue_id}");
            }
            return Ok(());
        };

        let state_id = self.state_id(name).await?;
        let data: IssueUpdateData = self
            .graphql(UPDATE, json!({ "id": issue_id, "stateId": state_id }))
            .await?;
        if !data.issue_update.success {
            bail!("linear issueUpdate failed setting state `{name}` on {issue_id}");
        }
        Ok(())
    }

    /// `Rejected`/`StaleClosed` both archive rather than move to a distinct state
    /// (see `decision_state_name`), so this can't tell them apart on read-back —
    /// either query returns every archived issue in the team. Not a problem
    /// today: nothing calls this with either variant (`Rejected` is set-only via
    /// `lucid task reject`, `StaleClosed` isn't implemented yet).
    async fn query_by_decision_state(
        &self,
        state: DecisionState,
    ) -> anyhow::Result<Vec<TrackerIssue>> {
        const QUERY: &str = r"
            query ByState($filter: IssueFilter!, $first: Int!, $after: String) {
              issues(
                filter: $filter
                first: $first
                after: $after
              ) {
                nodes { id title description state { name } archivedAt labels(first: 100) { nodes { id name } } }
                pageInfo { hasNextPage endCursor }
              }
            }
        ";

        let Some(state_name) = decision_state_name(state) else {
            return self.query_archived().await;
        };

        let mut filter = self.team_and_project_filter();
        filter["state"] = json!({ "name": { "eq": state_name } });

        // Callers use this for the rejected-state dedup check, where a truncated page
        // reads as "no match" and files a duplicate — see
        // docs/wiki/architecture/dedup-death-loop.md. Paginate fully.
        let mut collected = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let data: IssuesData = self
                .graphql(
                    QUERY,
                    json!({
                        "filter": filter,
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
            query Similar($term: String!, $filter: IssueFilter!, $first: Int!) {
              searchIssues(
                term: $term
                filter: $filter
                includeArchived: true
                first: $first
              ) {
                nodes { id title description state { name } archivedAt labels(first: 100) { nodes { id name } } }
              }
            }
        ";

        // Linear's own full-text+vector search, rather than a client-side title
        // heuristic: a reworded duplicate is exactly what the dedup check must catch.
        // Rate-limited to 30/min server-side; failures surface rather than retry.
        let data: SearchData = self
            .graphql(
                QUERY,
                json!({ "term": title, "filter": self.team_and_project_filter(), "first": 25 }),
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

fn team_id(data: &TeamsData) -> Option<String> {
    data.teams.nodes.first().map(|node| node.id.clone())
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
struct WorkflowStateName {
    name: String,
}

#[derive(Deserialize)]
struct IssueNode {
    id: String,
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    state: Option<WorkflowStateName>,
    #[serde(default, rename = "archivedAt")]
    archived_at: Option<String>,
    labels: Nodes<LabelNode>,
}

impl IssueNode {
    fn into_tracker_issue(self) -> TrackerIssue {
        // Archived issues (`Rejected`/`StaleClosed`, see `decision_state_name`)
        // read back as `Rejected` — the two are indistinguishable from Linear's
        // side; see `LinearAdapter::query_by_decision_state`'s doc comment.
        //
        // Archiving leaves the issue's prior state name untouched (Linear has no
        // "cleared" state), so a `Pending` proposal that got rejected still shows
        // `state.name == "Pending"` afterward. Only treat archive as incidental
        // (i.e. trust the state name) when that name is one of the three
        // positive/active outcomes lucid itself never archives *from* —
        // `Approved`/`Done`/`In Review` — which means an archived issue in one of
        // those states was very likely archived by a human tidying the board, not
        // by `lucid task reject`.
        let mapped_state = self
            .state
            .as_ref()
            .and_then(|s| decision_state_from_name(&s.name));
        let decision_state = if self.archived_at.is_some()
            && !matches!(
                mapped_state,
                Some(DecisionState::Approved | DecisionState::Done | DecisionState::NeedsReview)
            ) {
            Some(DecisionState::Rejected)
        } else {
            mapped_state
        };
        let review = self
            .labels
            .nodes
            .iter()
            .find_map(|node| review_from_label(&node.name))
            .unwrap_or(ReviewMode::Auto);
        TrackerIssue {
            id: self.id,
            title: self.title,
            // Linear returns `""` for a blank description, not `null` — normalize
            // so callers can match on `None` for "no handoff surface" uniformly.
            description: self.description.filter(|d| !d.is_empty()),
            decision_state,
            review,
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
struct ProjectsData {
    projects: Nodes<IdNode>,
}

#[derive(Deserialize)]
struct WorkflowStatesData {
    #[serde(rename = "workflowStates")]
    workflow_states: Nodes<IdNode>,
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
struct IssueUpdateData {
    #[serde(rename = "issueUpdate")]
    issue_update: SuccessPayload,
}

#[derive(Deserialize)]
struct IssueArchiveData {
    #[serde(rename = "issueArchive")]
    issue_archive: SuccessPayload,
}

#[derive(Deserialize)]
struct CommentCreateData {
    #[serde(rename = "commentCreate")]
    comment_create: SuccessPayload,
}

#[cfg(test)]
mod tests {
    use super::super::EffortEstimate;
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
            review: super::ReviewMode::Auto,
            verify_cmd: None,
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
    fn decision_state_names_round_trip_for_real_states() {
        for state in [
            DecisionState::Pending,
            DecisionState::Approved,
            DecisionState::Done,
            DecisionState::NeedsReview,
        ] {
            let name = decision_state_name(state).expect("real state");
            assert_eq!(decision_state_from_name(name), Some(state));
        }
        assert_eq!(decision_state_from_name("Triage"), None);
    }

    #[test]
    fn rejected_and_stale_have_no_state_name() {
        assert_eq!(decision_state_name(DecisionState::Rejected), None);
        assert_eq!(decision_state_name(DecisionState::StaleClosed), None);
    }

    #[test]
    fn issue_node_reads_decision_state_from_the_state_field() {
        let node: IssueNode = serde_json::from_value(json!({
            "id": "abc",
            "title": "Fix presence detection",
            "state": { "name": "Approved" },
            "labels": { "nodes": [
                { "id": "l1", "name": "area:presence" }
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
    fn project_filter_omitted_when_project_key_unset() {
        let adapter = LinearAdapter::new("key".to_string(), "ENG".to_string(), None);
        assert_eq!(
            adapter.team_and_project_filter(),
            json!({ "team": { "key": { "eq": "ENG" } } })
        );
    }

    #[test]
    fn project_filter_included_when_project_key_set() {
        let adapter = LinearAdapter::new(
            "key".to_string(),
            "ENG".to_string(),
            Some("Lucid".to_string()),
        );
        assert_eq!(
            adapter.team_and_project_filter(),
            json!({
                "team": { "key": { "eq": "ENG" } },
                "project": { "name": { "eq": "Lucid" } },
            })
        );
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
