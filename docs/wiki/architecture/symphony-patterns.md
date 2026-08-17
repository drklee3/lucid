# Symphony Patterns Borrowed Directly

OpenAI Symphony (SPEC.md + Elixir reference implementation — see [prior-art landscape](../research/prior-art-landscape.md)) is the closest existing system to lucid's Worker half, though it has no PM/proposal layer. Several of its mechanisms are worth reusing verbatim rather than re-deriving:

- **Orchestration state separate from tracker state.** Symphony tracks `Unclaimed → Claimed (Running|RetryQueued) → Released` internally, independent of whatever the tracker calls its statuses. The orchestrator owns the true state machine; the [tracker adapter](tracker-adapter.md) is just a projection. This is what makes tracker-swapping tractable.
- **Per-run phase tracking.** `PreparingWorkspace → BuildingPrompt → LaunchingAgentProcess → InitializingSession → StreamingTurn → Finishing → {Succeeded|Failed|TimedOut|Stalled|CanceledByReconciliation}`. Worth copying for the Worker's own logging/observability — see [state-machine gaps](state-machine-gaps.md) for where this needs extending, not just copying.
- **Continuation turns, not context resets.** "The first turn SHOULD use the full rendered task prompt. Continuation turns SHOULD send only continuation guidance to the existing thread." This is the fix for Symphony's own known weak point (review/rework loop treated as a full reset — see [review/rework UX](review-rework-ux.md)). When a human leaves PR feedback, the Worker resumes the *same* session (`claude -p --resume <session-id>` or equivalent) with just the review comments as the new turn — a hard requirement, not an optimization.
- **WORKFLOW.md frontmatter contract.** YAML frontmatter (tracker kind, active/terminal states, polling interval, `agent.max_turns`) + Markdown prompt body, strict template engine, fails loud on unknown variables. This shape is reused for lucid's own PM.md / RESEARCH.md / WORKER.md role contracts and for the [agent handoff surface](agent-handoff.md) embedded in tracker issues.
- **Workspace isolation invariants.** Per-issue workspace path, must stay inside workspace root (prefix-check on normalized absolute paths), sanitized workspace key with a hash suffix on collision. `after_create`/`before_run`/`after_run`/`before_remove` hooks, with only `before_run` failures aborting the run. Directly reusable for the Worker's isolated git-worktree checkouts.
- **Reconciliation tick.** Every tick: (1) stall-detect running agents (`elapsed_ms > stall_timeout_ms` since last event → kill + retry), (2) refresh tracker state for all claimed issues (terminal → cleanup, no-longer-active → stop without cleanup, still-active → update snapshot). This is the daemon-model backbone, adopted wholesale for the Worker executor loop.
- **Parked-state polling stop.** Once an issue is in `Human Review` (outside Symphony's `active_states`), Symphony stops dispatching/polling it entirely until a human moves it — cheap and correct. lucid's design implies the equivalent via decision-state gating but should make it an explicit rule for the Worker's own state machine, not just the PM's proposal flow — flagged in [state-machine gaps](state-machine-gaps.md).

## Broader architectural takeaways (from the wider prior-art survey, not Symphony alone)

1. **Orchestrator never codes** — every working system separates planning from implementation.
2. **Fresh contexts per sub-task** — no giant sessions; work decomposes into focused units.
3. **Independent validation** — the implementer never validates its own work.
4. **WORKFLOW.md pattern** — repo-owned agent contract versioned with code; universal praise across systems surveyed.
5. **Daemon model > CI-triggered** — a long-lived service with retries, reconciliation, stall detection beats a CI-job model.
