# lucid wiki index

An [LLM Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) (Karpathy pattern) — a persistent, compounding knowledge base, maintained directly rather than re-derived per query. See the root `CLAUDE.md` for the schema/conventions governing this wiki. Pages are organized by concept, one topic per page.

## Architecture (`architecture/`)

- [overview](architecture/overview.md) — system concept, core roles, key constraints, component diagram, the standalone-orchestrator correction
- [tech-stack](architecture/tech-stack.md) — why Rust, crate choices
- [presence-detection](architecture/presence-detection.md) — pluggable idle-source interface; verified `logind`-is-dead-on-WSL2 finding; append-only mode-transition audit log
- [tracker-adapter](architecture/tracker-adapter.md) — Linear as today's swappable tracker backend
- [agent-handoff](architecture/agent-handoff.md) — the frontmatter contract the Worker parses deterministically
- [harness-dispatch](architecture/harness-dispatch.md) — the (harness, auth-mode) profile list, subscription-first
- [harness-tracker-isolation](architecture/harness-tracker-isolation.md) — no dispatched harness ever gets a live tracker credential
- [pm-scope](architecture/pm-scope.md) — gap-detection, not open-ended ideation; PM investigation-on-wake scope
- [research-agent](architecture/research-agent.md) — tiered validation depth before the PM files a proposal
- [dedup-death-loop](architecture/dedup-death-loop.md) — the single most important piece of state in the system
- [symphony-patterns](architecture/symphony-patterns.md) — mechanisms borrowed directly from OpenAI Symphony
- [state-machine-gaps](architecture/state-machine-gaps.md) — missing states found via cross-system comparison
- [review-rework-ux](architecture/review-rework-ux.md) — the undecided auto-resume-vs-explicit-trigger fork
- [observability](architecture/observability.md) — mobile/notification UX, the deferred-dashboard decision, CLI-first v1
- [error-stall-visibility](architecture/error-stall-visibility.md) — the most consistent gap across every system surveyed
- [trace-correlation](architecture/trace-correlation.md) — tagging harness dispatches with OTel resource attributes so tracker items link straight to their trace
- [worker-completion](architecture/worker-completion.md) — per-issue git worktree isolation + `gh`-driven PR completion, `ReviewMode` (auto/human/agent) deciding who merges, and the presence-independent tick step that reconciles `NeedsReview` against the PR's own merge status
- [human-in-the-loop](architecture/human-in-the-loop.md) — ticket ingestion via the tracker itself, a pluggable `NotificationSink`, and the end-of-turn signal + session-resume design for mid-task questions that never blocks a process
- [ci-quality-tooling](architecture/ci-quality-tooling.md) — cargo-deny over cargo-audit, SHA-pinned actions, zizmor annotations over SARIF, weekly-only cargo-mutants
- [persistence](architecture/persistence.md) — flat-file-over-database convention across override_file, audit_log, FileTracker, and DaemonState; why rusqlite was removed unused; per-file corruption tolerance

## Research (`research/`)

- [prior-art-landscape](research/prior-art-landscape.md) — the full systems-survey table
- [pm-layer-novelty](research/pm-layer-novelty.md) — verdict: the proactive-PM layer is genuinely novel
- [practitioner-reality](research/practitioner-reality.md) — what actually happens running these systems unattended
- [risks-and-critiques](research/risks-and-critiques.md) — the strongest skeptical arguments, with rebuttals
- [matplotlib-incident](research/matplotlib-incident.md) — the sharpest documented no-per-action-review failure
- [presence-automation-prior-art](research/presence-automation-prior-art.md) — idle/presence-gating outside coding agents
- [open-questions](research/open-questions.md) — what the research pass left unresolved

## Living outside the wiki

- `docs/FEATURES.md` § Deferred / not v1 — the current, authoritative open-items list. Intentionally left outside the wiki as a fast-moving checklist rather than a wiki page, since it changes shape faster than the wiki should churn. Re-check it each time the wiki is queried for "what's next."
