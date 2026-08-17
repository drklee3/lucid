# Tracker Adapter

## Today's backend: Linear — chosen deliberately, not a foundational dependency

Linear's own remote MCP server (`mcp.linear.app`, OAuth2.1, full issue/project/comment CRUD) is directly usable by any MCP-capable client. Linear satisfies three requirements at once that a local-only store can't:

1. Single source of truth.
2. Remote/mobile management via Linear's native app.
3. Reuse of a mature product instead of building triage UI from scratch.

## Stays swappable by construction

PM/Worker code talks to a thin internal interface — `create_proposal`, `set_decision_state`, `query_by_label`, `query_similar` — never to Linear-specific API/label concepts directly. Same principle as Symphony's "orchestration state separate from tracker state" pattern (see [Symphony patterns](symphony-patterns.md)): the adapter maps *our* states to *its* labels, not the other way around. A GitHub Issues (or other) adapter is a second implementation of the same interface later, not a rewrite — and cheaper to design well as a *second* implementation once the interface shape is proven against real Linear use, rather than guessed abstractly with zero trackers wired up.

## Proposal format

Structured issue body: title, one-line summary, 2-3 bullet "why now," effort estimate (S/M/L), risk note, and a machine-readable YAML frontmatter block (see [agent handoff](agent-handoff.md) for the frontmatter contract itself). Decision surfaces via the adapter as whatever binary affordance the backend supports — Linear: label/state, e.g. `proposal:pending` → `proposal:approved` / `proposal:rejected`. Linear's mobile app renders that as a tappable action for free. No reaction after N days (recommend 7) auto-closes as stale, distinct from an explicit reject (matters for [dedup/death-loop prevention](dedup-death-loop.md)).

## No direct write access for dispatched harnesses

See [harness/tracker isolation](harness-tracker-isolation.md) — the orchestrator is the sole chokepoint for every tracker read and write. No coding harness (Claude Code, Codex, Hermes) ever holds a live Linear credential.

## `LinearAdapter` implementation notes

Grounded against Linear's published GraphQL SDK schema (`linear/linear` repo, `packages/sdk/src/schema.graphql`) and `linear.app/developers` — not implemented from memory, since a GraphQL schema is exactly the kind of thing that drifts. Concrete facts worth keeping current:

- **Endpoint/auth**: `https://api.linear.app/graphql`; a personal API key goes in a *bare* `Authorization` header — no `Bearer` prefix.
- **Env var**: `LINEAR_API_KEY`, the value `TrackerConfig::api_key_env` should name.
- **Team key required**: `issueCreate` needs a `teamId`, which the adapter resolves from Linear's short team key (e.g. `ENG`), not a UUID. This is config the original trait design didn't account for — `TrackerConfig` gained a `team_key: Option<String>` field (`Some` required when `backend = "linear"`) rather than changing `TrackerAdapter`'s trait signature itself, keeping the adapter-swap boundary from [Stays swappable by construction](#stays-swappable-by-construction) intact.
- **Label mutations use add/remove, not `issueUpdate{labelIds}`**: that input replaces the whole label set, which would silently drop any label a human added in the Linear app since the adapter's last read. `set_decision_state` diffs and issues `issueAddLabel`/`issueRemoveLabel` instead.
- **`query_by_label` paginates fully** via `pageInfo` rather than reading only the first page — a truncated page reading as "no match" would trigger exactly the duplicate-proposal death loop [dedup/death-loop prevention](dedup-death-loop.md) exists to prevent.
- **`query_similar` uses Linear's `searchIssues`** (real full-text+vector search), not a client-side title scan — rate-limited server-side to 30/min, no retry layer added on top; a rate-limit error surfaces to the caller rather than being silently swallowed.
- **`StaleClosed` maps to a `proposal:stale` label only** — it does not close or archive the Linear issue itself.
- **A missing `proposal:*` label errors rather than auto-creating it** — the adapter never mutates the workspace's label set; a missing label is treated as a setup error to fix in Linear directly.
- Plain typed client (`reqwest` + `serde_json`, no GraphQL codegen crate) — matches the deliberate choice already noted above of a plain client over Linear's MCP server.
