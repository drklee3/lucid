# No Direct Tracker Access for Dispatched Harnesses

The Worker's coding harness (`claude -p` / `codex exec` / `hermes -p`, whichever) never gets the Linear MCP (or any tracker credential) wired in directly — the same rule applies to the read-only reviewer dispatch `ReviewMode::Agent` runs (see [worker completion](worker-completion.md)). All tracker interaction — reads and writes alike — is mediated by the orchestrator through the [tracker adapter](tracker-adapter.md) interface.

**Reads**: the orchestrator resolves relevant context (related open issues, prior comments) via the adapter *before* dispatch and injects it into the prompt as plain text/structured summary. A harness never does its own live Linear query. If a long-running session needs fresher context mid-task, that's what the continuation-turn mechanism ([Symphony patterns](symphony-patterns.md)) is for — the orchestrator pushes updated context on resume, the harness doesn't pull it.

**Writes**: a harness never gets a "post comment" or "change status" tool. It produces structured output as part of its turn — a suggested comment, a status it believes is warranted, a "needs human input" signal (see [state-machine gaps](state-machine-gaps.md)) — and the *orchestrator* is the only thing that ever calls `set_decision_state` or posts a comment through the adapter.

## Why this matters — the matplotlib mitigation

This is a safety property, not just cleanliness: it's the concrete mitigation against the [matplotlib incident](../research/matplotlib-incident.md) failure mode — an agent with a lever over the outcome of its own review. No harness, on any tracker, ever has live write access to its own tracker item.

## Also a harness-agnosticism win

None of `claude -p`/`codex exec`/`hermes -p` need Linear (or any tracker) configured at all, so nothing tracker-specific needs to stay in sync across harnesses in [harness dispatch](harness-dispatch.md).
