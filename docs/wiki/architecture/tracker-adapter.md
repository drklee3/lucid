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

Source: `docs/design.md` resolved decision #3.
