# Tracker Adapter

## Today's backend: Linear — chosen deliberately, not a foundational dependency

Linear's own remote MCP server (`mcp.linear.app`, OAuth2.1, full issue/project/comment CRUD) is directly usable by any MCP-capable client. Linear satisfies three requirements at once that a local-only store can't:

1. Single source of truth.
2. Remote/mobile management via Linear's native app.
3. Reuse of a mature product instead of building triage UI from scratch.

## Stays swappable by construction

Worker/CLI code talks to a thin internal interface — `create_proposal`, `set_decision_state`, `query_by_decision_state` — never to Linear-specific API/label/state concepts directly. Same principle as Symphony's "orchestration state separate from tracker state" pattern (see [Symphony patterns](symphony-patterns.md)): the adapter maps *our* states to *its* backend representation, not the other way around. A GitHub Issues (or other) adapter is a second implementation of the same interface later, not a rewrite — and cheaper to design well as a *second* implementation once the interface shape is proven against real Linear use, rather than guessed abstractly with zero trackers wired up.

## Proposal format

Structured issue body: title, one-line summary, 2-3 bullet "why now," effort estimate (S/M/L), risk note, and a machine-readable YAML frontmatter block (see [agent handoff](agent-handoff.md) for the frontmatter contract itself). No reaction after N days (recommend 7) auto-closes as stale, distinct from an explicit reject.

## Decision state: the issue's real ticket state, not a label

`DecisionState` (`Pending`/`Approved`/`Rejected`/`StaleClosed`/`Done`/`NeedsReview`) moves the tracker's actual status field for a backend that has one — for `LinearAdapter` that means the issue's real `state` (the board-view/workflow column), mutated via `issueUpdate`'s `stateId`, not a `proposal:*` label. A task lucid closes shows up in the same `Done` column a human's own tickets use, is filterable and visible in Linear's board view like any other issue, and can drive Linear's own state-based automations — the point of building on a real project tracker instead of a bespoke store.

This requires four workflow states to exist by name in the team the adapter targets: `Pending`, `Approved`, `In Review`, `Done` — looked up via `state_id` (mirrors `label_id`'s "missing = workspace-setup error, the adapter never creates one" philosophy). `Done` commonly already exists in a team's default workflow and is reused as-is; the other three are typically new states created once per team.

`Rejected` and `StaleClosed` are **not** workflow states — they archive the issue (`issueArchive`) instead. The board-visibility argument that justifies real states for the other four inverts for these two: `Rejected`/`StaleClosed` are terminal "stop showing me this" outcomes, and a dedicated column for them would just clutter the board with declined proposals rather than surface anything worth seeing. Linear's own archive mechanism (drops the issue from default board/list views, reversible, doesn't touch its prior state field) is the better-fitting built-in tool.

`ReviewMode` (`review:auto`/`review:human`/`review:agent`) stays label-based, deliberately: it's a policy setting on the task (who's allowed to close it out), not a stage the task passes through, so it doesn't belong in the state field alongside the workflow's actual progress.

### Known limitation: `Rejected`/`StaleClosed` read-back is approximate

Archiving doesn't clear or touch the issue's prior `state` field — a `Pending` proposal that gets rejected still has `state.name == "Pending"` afterward, it just also carries `archivedAt`. Reading `decision_state` back out for an archived issue means:

- **`Rejected` and `StaleClosed` are indistinguishable.** Both archive; nothing on the Linear side records *which* of the two caused it. `query_by_decision_state` treats every qualifying archived issue as `Rejected` regardless of which variant produced it. Not a problem today: nothing in the codebase queries for either variant except `lucid task list --state rejected`, and `StaleClosed` isn't implemented (no auto-stale-close exists yet — see `docs/FEATURES.md`).
- **An archived issue whose prior state was `Approved`/`Done`/`In Review` is treated as *not* rejected** — those three are outcomes lucid never archives *from* (only `Pending`, and in principle a not-yet-approved state, go through `lucid task reject`), so an archived issue sitting in one of them is read as "a human tidied up the board after the fact," not as a lucid rejection. This is a heuristic, not a guarantee — a future `lucid task reject` call against an already-approved issue would misclassify on read-back.
- **`query_by_decision_state(Rejected)` has no way to scope to lucid-managed issues only, unless `managed_label` is configured.** It queries every archived issue in the team and keeps the ones that read as `Rejected` by the heuristic above — including issues archived by a human for reasons that have nothing to do with lucid at all, from long before lucid was ever wired up to the team. Confirmed live: a query against a real team surfaced an unrelated, years-old archived ticket alongside a genuine `lucid task reject` result. Fine for today's only consumer (`lucid task list --state rejected`, a human-facing convenience view) when `managed_label` is unset; setting `TrackerConfig::managed_label` (see [Managed-label scoping](#managed-label-scoping-closing-the-scope-leak) below) narrows this properly since `query_archived` shares `team_and_project_filter` with every other query.

## Managed-label scoping: closing the scope leak

Team+project scoping (`team_and_project_filter`) narrows lucid's queries to a corner of a Linear workspace, but a human can still move any issue in that same team/project into the exact workflow state (e.g. `Approved`) lucid polls for reasons that have nothing to do with lucid — that issue would then get picked up and dispatched as if it were a lucid proposal. Team/project scoping alone can't distinguish "an issue lucid filed" from "an issue a human happens to have sitting in the same state."

`TrackerConfig::managed_label` (e.g. `"lucid"`) closes this: when set, `LinearAdapter::team_and_project_filter` additionally requires that label, so `query_by_decision_state` and `query_archived` (both built on `team_and_project_filter`) only ever see issues carrying it. `create_proposal` attaches the label automatically alongside the existing `review:*` label, so every lucid-filed issue is self-scoping without an operator needing to label things by hand.

Filtering matches by label *name* (`labels: { some: { name: { eq } } }`), not by resolved id — unlike `labelIds` on a mutation, Linear's `IssueFilter` takes a label name directly, so reads don't need the extra `label_id` round-trip; only `create_proposal`'s label attachment needs the resolved id, same as any other `labelIds` write. Like `team_key`/`review:*`/workflow-state labels, lucid never creates `managed_label` — a missing one is a workspace-setup error surfaced at first use (`label_id`'s existing failure mode), not something silently created.

`None` (the default) preserves today's exact behavior — team/project scoping only, no label filter — so existing single-tenant setups need no config change.

## Structured attachments: `attach_link` vs `attach_note`

`TrackerAdapter::attach_link(issue_id, title, url)` posts a structured title+url attachment, distinct from `attach_note`'s plain-text comment. `LinearAdapter::attach_link` calls Linear's real `attachmentCreate` GraphQL mutation, so the link shows up as a rich attachment in Linear's UI rather than a line buried in a note's body. `FileTracker::attach_link` has no structured-attachment concept to map onto — it appends a `[attachment] {title}: {url}` entry to the same `notes` field `attach_note` uses, since `FileTracker` is a local stand-in backend, not the real target of this feature.

The Worker uses this for the OTel trace link posted at dispatch completion — see [trace correlation](trace-correlation.md#writing-the-link-back-a-structured-attachment-not-comment-text). The dispatch-status note posted alongside it no longer repeats the link as text now that it's a structured attachment.

## PR linking: a GitHub magic word, not a lucid-created attachment

`TrackerIssue` carries an `identifier: Option<String>` — Linear's human-readable ID (e.g. `SUSHI-72`), distinct from `id` (the internal UUID). `LinearAdapter` populates it from the `identifier` field on every issue-fetching GraphQL query (`query_by_decision_state`, the archived-issues query); it's always `None` for `FileTracker`, which has no separate human-readable form.

`worker::open_pr`'s PR body leads with `Fixes <reference>` (the `pr_body` helper in `src/worker.rs`), using `issue.identifier` when present and falling back to the raw `issue.id` otherwise. Linear's installed GitHub integration recognizes this magic word and auto-links the PR as a rich attachment (diffs, checks, review sync) on its own — lucid does **not** call `attachmentCreate` for the PR link itself; only `attach_link`'s trace-link use (above) is a lucid-created attachment. This is why `identifier` needed to exist at all: `Fixes <internal-uuid>` wouldn't mean anything to Linear's integration, which matches on the team's short key + number form.

## No direct write access for dispatched harnesses

See [harness/tracker isolation](harness-tracker-isolation.md) — the orchestrator is the sole chokepoint for every tracker read and write. No coding harness (Claude Code, Codex, Hermes) ever holds a live Linear credential.

## `LinearAdapter` implementation notes

Grounded against Linear's published GraphQL SDK schema (`linear/linear` repo, `packages/sdk/src/schema.graphql`) and `linear.app/developers` — not implemented from memory, since a GraphQL schema is exactly the kind of thing that drifts. Concrete facts worth keeping current:

- **Endpoint/auth**: `https://api.linear.app/graphql`; a personal API key goes in a *bare* `Authorization` header — no `Bearer` prefix.
- **Env var**: `LINEAR_API_KEY`, the value `TrackerConfig::api_key_env` should name.
- **Team key required**: `issueCreate` needs a `teamId`, which the adapter resolves from Linear's short team key (e.g. `ENG`), not a UUID. This is config the original trait design didn't account for — `TrackerConfig` gained a `team_key: Option<String>` field (`Some` required when `backend = "linear"`) rather than changing `TrackerAdapter`'s trait signature itself, keeping the adapter-swap boundary from [Stays swappable by construction](#stays-swappable-by-construction) intact.
- **Project scope is optional**: Linear issues don't require a project, so `TrackerConfig::project_key: Option<String>` (a project *name*, resolved to id via a `projects(filter: { name: { eq } })` lookup) narrows the adapter to one project within `team_key` when set, and leaves it team-wide when `None`. Read queries (`query_by_decision_state`, the archived-issues query) build the team/project filter as a JSON `Value` (`team_and_project_filter`) rather than passing `$project` as a nullable GraphQL variable — a null-valued `eq` filter would match issues with a null project name, i.e. nothing, instead of disabling the clause.
- **Label scope is optional too**: `TrackerConfig::managed_label: Option<String>` adds a `labels: { some: { name: { eq } } }` clause to the same `team_and_project_filter` when set — see [Managed-label scoping](#managed-label-scoping-closing-the-scope-leak).
- **Decision state moves via a single `issueUpdate{stateId}`** for `Pending`/`Approved`/`In Review`/`Done` — unlike labels, `state` is a single-valued field on the issue, so there's no add/remove diffing needed the way `review:*` labels require; setting `stateId` simply replaces it. `Rejected`/`StaleClosed` instead call `issueArchive{id}` — see [Known limitation](#known-limitation-rejectedstaleclosed-read-back-is-approximate) above.
- **`review:*` label mutations use add/remove, not `issueUpdate{labelIds}`**: that input replaces the whole label set, which would silently drop any label a human added in the Linear app since the adapter's last read.
- **`query_by_decision_state` paginates fully** via `pageInfo` rather than reading only the first page — a truncated page reading as "no match" would make an `Approved` issue silently invisible to the dispatch loop. For `Rejected`/`StaleClosed` it delegates to a separate archived-issues query (`includeArchived: true`) rather than the state-name filter, since neither has a state name to filter on.
- **A missing named workflow state or `review:*` label errors rather than auto-creating it** — the adapter never mutates the workspace's state/label set; a missing one is treated as a setup error to fix in Linear directly. `WorkflowStateFilter`/`IssueFilter.state` shapes were confirmed by introspecting the live GraphQL schema, not assumed from `IssueLabelFilter`'s (matching) shape.
- Plain typed client (`reqwest` + `serde_json`, no GraphQL codegen crate) — matches the deliberate choice already noted above of a plain client over Linear's MCP server.
