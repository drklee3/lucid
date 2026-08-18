# Tracker Adapter

## Today's backend: Linear — chosen deliberately, not a foundational dependency

Linear's own remote MCP server (`mcp.linear.app`, OAuth2.1, full issue/project/comment CRUD) is directly usable by any MCP-capable client. Linear satisfies three requirements at once that a local-only store can't:

1. Single source of truth.
2. Remote/mobile management via Linear's native app.
3. Reuse of a mature product instead of building triage UI from scratch.

## Stays swappable by construction

PM/Worker code talks to a thin internal interface — `create_proposal`, `set_decision_state`, `query_by_decision_state`, `query_similar` — never to Linear-specific API/label/state concepts directly. Same principle as Symphony's "orchestration state separate from tracker state" pattern (see [Symphony patterns](symphony-patterns.md)): the adapter maps *our* states to *its* backend representation, not the other way around. A GitHub Issues (or other) adapter is a second implementation of the same interface later, not a rewrite — and cheaper to design well as a *second* implementation once the interface shape is proven against real Linear use, rather than guessed abstractly with zero trackers wired up.

## Proposal format

Structured issue body: title, one-line summary, 2-3 bullet "why now," effort estimate (S/M/L), risk note, and a machine-readable YAML frontmatter block (see [agent handoff](agent-handoff.md) for the frontmatter contract itself). No reaction after N days (recommend 7) auto-closes as stale, distinct from an explicit reject (matters for [dedup/death-loop prevention](dedup-death-loop.md)).

## Decision state: the issue's real ticket state, not a label

`DecisionState` (`Pending`/`Approved`/`Rejected`/`StaleClosed`/`Done`/`NeedsReview`) moves the tracker's actual status field for a backend that has one — for `LinearAdapter` that means the issue's real `state` (the board-view/workflow column), mutated via `issueUpdate`'s `stateId`, not a `proposal:*` label. A task lucid closes shows up in the same `Done` column a human's own tickets use, is filterable and visible in Linear's board view like any other issue, and can drive Linear's own state-based automations — the point of building on a real project tracker instead of a bespoke store.

This requires four workflow states to exist by name in the team the adapter targets: `Pending`, `Approved`, `In Review`, `Done` — looked up via `state_id` (mirrors `label_id`'s "missing = workspace-setup error, the adapter never creates one" philosophy). `Done` commonly already exists in a team's default workflow and is reused as-is; the other three are typically new states created once per team.

`Rejected` and `StaleClosed` are **not** workflow states — they archive the issue (`issueArchive`) instead. The board-visibility argument that justifies real states for the other four inverts for these two: `Rejected`/`StaleClosed` are terminal "stop showing me this" outcomes, and a dedicated column for them would just clutter the board with declined proposals rather than surface anything worth seeing. Linear's own archive mechanism (drops the issue from default board/list views, reversible, doesn't touch its prior state field) is the better-fitting built-in tool. `query_similar` already passes `includeArchived: true`, so an archived issue still gets caught by the dedup check against a reworded resubmission.

`ReviewMode` (`review:auto`/`review:human`/`review:agent`) stays label-based, deliberately: it's a policy setting on the task (who's allowed to close it out), not a stage the task passes through, so it doesn't belong in the state field alongside the workflow's actual progress.

### Known limitation: `Rejected`/`StaleClosed` read-back is approximate

Archiving doesn't clear or touch the issue's prior `state` field — a `Pending` proposal that gets rejected still has `state.name == "Pending"` afterward, it just also carries `archivedAt`. Reading `decision_state` back out for an archived issue means:

- **`Rejected` and `StaleClosed` are indistinguishable.** Both archive; nothing on the Linear side records *which* of the two caused it. `query_by_decision_state` treats every qualifying archived issue as `Rejected` regardless of which variant produced it. Not a problem today: nothing in the codebase queries for either variant except `lucid task list --state rejected`, and `StaleClosed` isn't implemented (no auto-stale-close exists yet — see `docs/FEATURES.md`).
- **An archived issue whose prior state was `Approved`/`Done`/`In Review` is treated as *not* rejected** — those three are outcomes lucid never archives *from* (only `Pending`, and in principle a not-yet-approved state, go through `lucid task reject`), so an archived issue sitting in one of them is read as "a human tidied up the board after the fact," not as a lucid rejection. This is a heuristic, not a guarantee — a future `lucid task reject` call against an already-approved issue would misclassify on read-back.
- **`query_by_decision_state(Rejected)` has no way to scope to lucid-managed issues only.** It queries every archived issue in the team and keeps the ones that read as `Rejected` by the heuristic above — including issues archived by a human for reasons that have nothing to do with lucid at all, from long before lucid was ever wired up to the team. Confirmed live: a query against a real team surfaced an unrelated, years-old archived ticket alongside a genuine `lucid task reject` result. Fine for today's only consumer (`lucid task list --state rejected`, a human-facing convenience view), not something to build automated logic on without narrowing it first (e.g. a `lucid`-authored marker in the issue, once one exists).

## No direct write access for dispatched harnesses

See [harness/tracker isolation](harness-tracker-isolation.md) — the orchestrator is the sole chokepoint for every tracker read and write. No coding harness (Claude Code, Codex, Hermes) ever holds a live Linear credential.

## `LinearAdapter` implementation notes

Grounded against Linear's published GraphQL SDK schema (`linear/linear` repo, `packages/sdk/src/schema.graphql`) and `linear.app/developers` — not implemented from memory, since a GraphQL schema is exactly the kind of thing that drifts. Concrete facts worth keeping current:

- **Endpoint/auth**: `https://api.linear.app/graphql`; a personal API key goes in a *bare* `Authorization` header — no `Bearer` prefix.
- **Env var**: `LINEAR_API_KEY`, the value `TrackerConfig::api_key_env` should name.
- **Team key required**: `issueCreate` needs a `teamId`, which the adapter resolves from Linear's short team key (e.g. `ENG`), not a UUID. This is config the original trait design didn't account for — `TrackerConfig` gained a `team_key: Option<String>` field (`Some` required when `backend = "linear"`) rather than changing `TrackerAdapter`'s trait signature itself, keeping the adapter-swap boundary from [Stays swappable by construction](#stays-swappable-by-construction) intact.
- **Project scope is optional**: Linear issues don't require a project, so `TrackerConfig::project_key: Option<String>` (a project *name*, resolved to id via a `projects(filter: { name: { eq } })` lookup) narrows the adapter to one project within `team_key` when set, and leaves it team-wide when `None`. Read queries (`query_by_decision_state`, `query_similar`, the archived-issues query) build the team/project filter as a JSON `Value` (`team_and_project_filter`) rather than passing `$project` as a nullable GraphQL variable — a null-valued `eq` filter would match issues with a null project name, i.e. nothing, instead of disabling the clause.
- **Decision state moves via a single `issueUpdate{stateId}`** for `Pending`/`Approved`/`In Review`/`Done` — unlike labels, `state` is a single-valued field on the issue, so there's no add/remove diffing needed the way `review:*` labels require; setting `stateId` simply replaces it. `Rejected`/`StaleClosed` instead call `issueArchive{id}` — see [Known limitation](#known-limitation-rejectedstaleclosed-read-back-is-approximate) above.
- **`review:*` label mutations use add/remove, not `issueUpdate{labelIds}`**: that input replaces the whole label set, which would silently drop any label a human added in the Linear app since the adapter's last read.
- **`query_by_decision_state` paginates fully** via `pageInfo` rather than reading only the first page — a truncated page reading as "no match" would trigger exactly the duplicate-proposal death loop [dedup/death-loop prevention](dedup-death-loop.md) exists to prevent. For `Rejected`/`StaleClosed` it delegates to a separate archived-issues query (`includeArchived: true`) rather than the state-name filter, since neither has a state name to filter on.
- **`query_similar` uses Linear's `searchIssues`** (real full-text+vector search), not a client-side title scan — rate-limited server-side to 30/min, no retry layer added on top; a rate-limit error surfaces to the caller rather than being silently swallowed.
- **A missing named workflow state or `review:*` label errors rather than auto-creating it** — the adapter never mutates the workspace's state/label set; a missing one is treated as a setup error to fix in Linear directly. `WorkflowStateFilter`/`IssueFilter.state` shapes were confirmed by introspecting the live GraphQL schema, not assumed from `IssueLabelFilter`'s (matching) shape.
- Plain typed client (`reqwest` + `serde_json`, no GraphQL codegen crate) — matches the deliberate choice already noted above of a plain client over Linear's MCP server.
