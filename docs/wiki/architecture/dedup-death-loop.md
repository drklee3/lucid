# Dedup / Death-Loop Prevention

Before filing, the PM checks three things:

1. Open Linear issues with matching content hash/title similarity — queried **live via MCP**, not a local mirror.
2. Issues in the `Rejected` decision state (👎'd, or auto-stale-closed within N days — recommend 30) — for `LinearAdapter` this archives the issue rather than moving it to a label or a workflow state; `query_similar`'s `includeArchived: true` still surfaces it. See [Tracker adapter](tracker-adapter.md#decision-state-the-issues-real-ticket-state-not-a-label).
3. Open PRs touching the same files.

Any hit blocks filing. Linear itself is the source of truth for this check — no separate local dedup store to keep in sync or lose.

**This is the single most important piece of state in the whole system** — losing it silently reintroduces every idea a human already said no to. Design implication: don't build a local cache of this as an optimization without a very good reason; the live-query property is load-bearing, not incidental.
