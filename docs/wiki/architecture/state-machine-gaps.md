# State-Machine Gaps (vs. Prior Art)

Grounded via a cross-system comparison (Symphony, Linear Coding Sessions, cyrus, GitHub Copilot coding agent, Factory Missions, OpenHands, Cursor). Gap list — what an existing system does that lucid's design didn't originally address.

## No "awaiting human input" state, distinct from blocked/stalled

Linear's Agent Sessions has a dedicated `awaitingInput` status driven by an `elicitation` activity type — the agent explicitly pauses mid-task to ask a clarifying question, and a reply resumes it via a `prompted` webhook. lucid's phase list (borrowed from [Symphony](symphony-patterns.md)) has no equivalent — a Worker that needs to ask something has nowhere to go but stall-detection or a generic error. Real gap if the Worker should ask a scoping question rather than guess and ship wrong.

## No loop/unproductive-progress detection, only elapsed-time stall detection

OpenHands has a first-class `STUCK` terminal state, distinct from generic `ERROR` (which its own source marks "optional for future use" — even OpenHands hasn't fully solved this). Symphony's `Stalled` is purely `elapsed_ms > stall_timeout_ms` — silence, not unproductive activity. Practitioner research (see [practitioner reality](../research/practitioner-reality.md), the "while loop" HN thread) found agents that don't go silent, they just loop on low-value busywork (rewriting tests, endless TODO.md updates) — a time-based stall timer never fires for that. No design answer for this failure mode yet.

## Parked-state polling stop needs to be explicit

Symphony's pattern — once an issue is in `Human Review` (outside `active_states`), stop dispatching/polling it entirely until a human moves it — is good and should be a stated rule for the Worker's own state machine, not just implied via decision-state gating on the PM's proposal flow. See [Symphony patterns](symphony-patterns.md).

## Related

- [Review/rework UX](review-rework-ux.md) — the trigger-mechanism fork this state machine needs to support
- [Error/stall visibility](error-stall-visibility.md) — what happens when these states aren't surfaced

Source: `docs/design.md` § UX / State-Machine Gap Analysis → State machine.
