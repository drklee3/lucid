# Observability, Mobile/Notification UX, Dashboard

## Mobile/notification UX

- **Resolved, not a gap**: PR review happens natively in GitHub (its own mature mobile review flow — filter/sort sessions, review, merge-conflict fixes from GitHub Mobile), not in Linear. Linear's role is scoped to the proposal/gap-flag layer only (issue-level label toggle), which its mobile app already handles fine. Clean separation: Linear = task/proposal tracking, GitHub = code review.
- **Push notification granularity is undocumented on Linear's side.** The assumption that "Linear mobile push notifications" solve the remote-visibility requirement isn't fully confirmed — Linear's own docs don't confirm distinct push events for "agent done" vs. "agent stuck" vs. "PR ready," only a generic "you'll be notified when input is needed" claim. Cursor's native iOS app is more explicit here (documented push on "work ready for review"). Worth a direct check before relying on Linear's mobile notifications for the "stuck at 3am" signal.
- Factory's "shareable session link, zero-install, watch or take over" pattern has no equivalent in this design — not necessarily needed for a single-user system, but a deliberately-skipped feature, not an unnoticed gap.

## Dashboard: deferred by decision, not by default

No dashboard plan exists for v1, and that's an explicit decision, not an oversight. Every system surveyed except cyrus (self-hosted) has *something*: Symphony's optional LiveView dashboard (Blocked/Retrying/Running tables, color-coded badges, token/runtime metrics — though blocked-state is in-memory only and lost on restart, a limitation worth not repeating), Copilot's session-log transcript viewer with an `Agent-Logs-Url` commit trailer for audit, OpenHands' full live IDE+terminal+browser view (the most immersive of anything surveyed), Cursor's real-time progress + diff viewer + attached videos/screenshots/logs on the PR itself.

**v1 observability is CLI-only**, modeled on the same content Symphony's dashboard shows (running/blocked/retrying agents, see [Symphony patterns](symphony-patterns.md)) but rendered as terminal output: one command starts the daemon, a second lists/inspects active agents — `ps`-for-agents, Symphony's Blocked/Retrying/Running table content as CLI output. Linear (see [tracker adapter](tracker-adapter.md)) is the async, periodic check-in surface (proposal review, gap-flags); the CLI is for live "what's happening right now." A web dashboard becomes a real ask once/if CLI-checking-in becomes the bottleneck, not before.

Concrete low-cost path for whenever the dashboard is built: Symphony's dashboard content model (Blocked table with last-error, Retry Queue table, Running table, color-coded state badges) is a fully specified, proven-rough draft — worth adopting the shape rather than designing one from scratch. `Daemon.runs` already survives a restart (unlike Symphony's own in-memory blocked map) via `DaemonState`'s flat-file persistence — see [persistence](persistence.md) — so a future dashboard would read from state that's already durable, not need to add durability itself.

## Proof-of-work artifacts

Symphony's stated goal — "CI status, PR review feedback, complexity analysis, and walkthrough videos" attached to the tracker item so a human can approve without re-running anything — and Cursor's pattern of attaching videos/screenshots/logs to the PR are both patterns lucid hasn't adopted yet. Worth adding to the Worker's PR-completion behavior. This stays relevant even with the dashboard deferred, since it's about what lands on the GitHub PR / Linear issue itself, not a dashboard feature.

One concrete instance of this: see [trace correlation](trace-correlation.md) — tagging each harness dispatch with an OTel resource attribute (`ticket_id` + `dispatch_id`) and posting the resulting trace-query link back to the tracker item, so "why did this run go wrong" is answerable from the ticket itself.
