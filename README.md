# lucid

Autonomous, presence-aware development orchestration. Named for lucid dreaming —
acts on its own, but stays directed within bounds you set; never fully unsupervised.

A standalone Rust daemon that, while you're away (presence-gated, not on a naive
schedule), investigates a repo against a stated goal, flags concrete gaps as
proposals for you to approve, and dispatches approved work to a coding harness
(`claude -p`, `codex exec`, or others) in an isolated git worktree — reconciling
state, retrying, and reporting back the same way it would if you were watching.

Tracker-agnostic, harness-agnostic, model-agnostic by design. See `docs/wiki/index.md`
for the full architecture, resolved decisions, and grounding research.

## Status

Design/brainstorming phase. No code yet.
