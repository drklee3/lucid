# System Overview

lucid is a multi-agent orchestration system whose agents don't just execute tasks but **proactively investigate**, **propose work**, and **continue development** while the user is away. It's composed of open-source building blocks — agnostic to tracker, coding harness, and model.

## Core roles

1. **PM Agent** — investigates the repo, identifies gaps against a stated goal, proposes tasks. Hands off to the Research agent for validation. Files proposals as tracker issues. This is the differentiated piece — see [PM scope](pm-scope.md) and [prior-art landscape](../research/prior-art-landscape.md) for why nothing existing covers it.
2. **Research Agent** — validates a proposed task (feasibility, prior art, dependency compatibility) before the PM files it. See [research agent](research-agent.md).
3. **Worker Agent** — monitors the tracker for approved tasks, executes via a coding harness, opens PRs. This half exists in many forms already (see [prior-art landscape](../research/prior-art-landscape.md)); the novelty here is the PM layer above it, not the Worker.
4. **Human** — reviews proposals as binary decisions, reviews PRs. The system respects presence: when the user is at the keyboard, they drive; when idle, it continues autonomously. See [presence detection](presence-detection.md).

## Key constraints

- **Tracker-agnostic** — not locked to Linear. See [tracker adapter](tracker-adapter.md).
- **Harness-agnostic** — not locked to any one coding CLI. See [harness dispatch](harness-dispatch.md).
- **Model-agnostic** — each role can use a different model.
- **Presence-aware trigger** — not a naive cron. See [presence detection](presence-detection.md).

## Architectural correction: standalone, not embedded in Hermes

An early draft of this design made Hermes (the user's existing always-on agent) the host process — a "PM bot profile" living inside Hermes, dispatching `claude -p` from there. That was reconsidered: the orchestration core (poll, dispatch, track state, retry, reconcile) is a **deterministic control loop** — a systems-engineering problem, not an LLM-reasoning one — and doesn't need or benefit from running inside an agent framework.

The corrected shape: lucid is its own small standalone process. Hermes becomes one interchangeable *harness backend* (`hermes -p "task"`), on equal footing with `claude -p` / `codex exec`, not the foundation everything else sits on. This is what "harness-agnostic" already implied — embedding in Hermes would have quietly violated that constraint by making Hermes load-bearing instead of swappable. See [tech stack](tech-stack.md) for the resulting implementation choice (Rust, not Python/Hermes-hosted).

## Components

```
Standalone orchestrator (Rust, its own always-on process)
  │
  ├── Presence watcher       → gates whether PM/Worker loops are allowed to act
  │                            (see presence-detection.md)
  ├── PM gap-detection job   → reads watermark + goal doc + git log + tracker,
  │                            emits gap-flag stubs (see pm-scope.md)
  ├── Research agent         → validates a proposal before filing (see research-agent.md)
  ├── Tracker adapter        → thin interface, Linear is the v1 implementation
  │                            (see tracker-adapter.md)
  ├── Worker executor        → per-issue git worktree isolation, harness dispatch,
  │                            continuation-turn handling (see harness-dispatch.md)
  └── Reconciliation loop    → Symphony-style poll tick: stall-detect, refresh
                               tracker state, requeue/cleanup (see symphony-patterns.md)
```

Almost everything here is net-new code, deliberately — each piece is small and well-scoped (a poll loop, a D-Bus watcher, a 4-method tracker adapter, git-worktree management, subprocess dispatch), not a framework. Symphony proves this shape works as a lean standalone daemon.

## Related pages

- [Tech stack](tech-stack.md) — why Rust
- [Presence detection](presence-detection.md)
- [Tracker adapter](tracker-adapter.md)
- [Harness dispatch](harness-dispatch.md)
- [PM scope](pm-scope.md)
- [Symphony patterns](symphony-patterns.md) — what was borrowed directly from prior art
- [Prior-art landscape](../research/prior-art-landscape.md) — why the PM layer specifically is the novel piece
