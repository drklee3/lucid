# Review/Rework UX — an Undecided Fork in the Road

## The fork

cyrus and GitHub Copilot coding agent made opposite defaults, both deliberately:

- **cyrus** auto-resumes on *any* PR "Request Changes" review (webhook on `pull_request_review`, no mention needed) and on any Linear thread reply.
- **Copilot** explicitly does **not** act on native review comments — only on an explicit `@copilot` mention. A documented, deliberate safety change; their changelog frames it as preventing the agent from "interpreting notes as commands."

lucid's resolved decision is "continuation turns, not resets" (correct, and better than Symphony's own reset-on-Rework flow — see [Symphony patterns](symphony-patterns.md)) but that decision never picked a side on *auto-resume vs. explicit-trigger*. Given the [matplotlib incident](../research/matplotlib-incident.md), this is worth deciding deliberately rather than defaulting to whichever is easier to build. **Still open.**

## The cautionary tale for however it's built

cyrus's webhook-driven continuation is the most automatic of anything surveyed, but its own open issue tracker documents session-identity bugs adjacent to this exact mechanism. Cursor has the same failure independently: GitHub-issue-triggered follow-ups sometimes spin up a *new* session instead of resuming the old one, breaking the whole point of continuation. Whatever trigger lucid builds needs an explicit, tested session-identity key — not an assumption that "webhook fires → resume the right thread" just works.

## Merge conflicts: unsolved industry-wide

No system reviewed has solved this. Copilot explicitly does not auto-rebase; its "Fix with Copilot" one-click resolver is unreliable per multiple 2026 community threads. Symphony's WORKFLOW.md sidesteps the problem rather than solving it — `Rework` does a hard reset (close PR, fresh branch off `origin/main`) specifically so it never has to rebase an old branch. lucid's "continuation turns" decision is better for context/review economy but doesn't have Symphony's out — if a continuation-based Worker's branch goes stale relative to `main`, there's no answer yet, and neither does anyone else surveyed. Flag as unsolved-industry-wide, don't assume the current approach will just handle it.

Source: `docs/design.md` § UX / State-Machine Gap Analysis → Review/rework UX.
