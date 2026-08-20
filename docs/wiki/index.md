# lucid wiki index

**Internal design notes, not user documentation** — looking for how to run or configure lucid? See [`docs/README.md`](../README.md) instead. This is the *why*, not the *how*: architecture decisions, research, open questions, and resolved-vs-still-undecided calls, side by side, exactly as they were reasoned through.

An [LLM Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) (Karpathy pattern) — a persistent, compounding knowledge base, maintained directly rather than re-derived per query. See the root `CLAUDE.md` for the schema/conventions governing this wiki. Pages are organized by concept, one topic per page.

## Architecture (`architecture/`)

Grouped by primitive category — see [overview](architecture/overview.md) § Components, by category for what these categories mean and how they relate.

**Start here**: [overview](architecture/overview.md) — system concept, core roles, key constraints, the categorized component diagram, the standalone-orchestrator correction. [tech-stack](architecture/tech-stack.md) — why Rust, crate choices.

**Pipeline stages** (the daemon's own control flow, not swappable):
- [agent-handoff](architecture/agent-handoff.md) — the frontmatter contract the Worker parses deterministically
- [worker-completion](architecture/worker-completion.md) — per-issue git worktree isolation + `gh`-driven PR completion, `ReviewMode` (auto/human/agent) deciding who merges, and the presence-independent tick step that reconciles `NeedsReview` against the PR's own merge status
- [symphony-patterns](architecture/symphony-patterns.md) — mechanisms borrowed directly from OpenAI Symphony
- [state-machine-gaps](architecture/state-machine-gaps.md) — missing states found via cross-system comparison
- [review-rework-ux](architecture/review-rework-ux.md) — the undecided auto-resume-vs-explicit-trigger fork

**Pluggable backends** (trait + config-selected implementation — `TrackerAdapter`, `PresenceSource`, `ExecutionBackend`, `NotificationSink`):
- [tracker-adapter](architecture/tracker-adapter.md) — Linear as today's swappable tracker backend; `attach_link` structured attachments vs `attach_note` comments; `identifier`-driven GitHub PR magic-word linking
- [harness-tracker-isolation](architecture/harness-tracker-isolation.md) — no dispatched harness ever gets a live tracker credential
- [presence-detection](architecture/presence-detection.md) — pluggable idle-source interface; verified `logind`-is-dead-on-WSL2 finding; append-only mode-transition audit log
- [harness-dispatch](architecture/harness-dispatch.md) — the (harness, auth-mode) profile list, subscription-first
- [sandboxed-execution](architecture/sandboxed-execution.md) — sandboxed-by-default dispatch: `Sandboxed` profiles run in a real `lucid-sandbox:latest` Docker container (worktree + git-common-dir mounted, nothing else), the `--dangerously-*`-style opt-out for running on the bare host, and why isolation is what unlocks parallel dispatch
- [human-in-the-loop](architecture/human-in-the-loop.md) — ticket ingestion via the tracker itself, `NotificationSink`/`ScriptSink` (shipped), and the end-of-turn signal + session-resume design for mid-task questions that never blocks a process

**Extensibility** (script-backed implementations layered on the pluggable-backend pattern — one mechanism, not a separate hooks system):
- [extensibility-primitives](architecture/extensibility-primitives.md) — the governing rule for when a new capability should default to being scriptable, `verify_cmd`/`HarnessProfile.cmd` as the already-shipped precedent, the `ScriptSink` pilot (built), the one-shot/persistent invocation split, the restricted-JSON-RPC-2.0 wire protocol (grounded via `research-first`), JSON Schema + `schemars` versioning
- [ux-principles](architecture/ux-principles.md) — lawsofux.com's 30 UX laws evaluated against lucid's CLI/notification and extension-author surfaces; which concretely apply (Jakob's Law, Postel's Law, Tesler's Law, Hick's Law, Doherty Threshold...), which are adapted-from-visual, and which don't apply at all (Fitts's Law)

**Cross-cutting** (not owned by any one category above):
- [multi-project](architecture/multi-project.md) — one daemon instance managing many repos: `Daemon::tick()` loops every configured `ProjectRuntime` sequentially with one global presence resolution, repo-owned `lucid.project.toml` per-project config (Symphony's `WORKFLOW.md` pattern adapted), directory-detected `--project` CLI flag across the `task` subcommands (real overrides on `dispatch-now`, flag-acceptance-and-validation only on `list`/`create`/`approve`/`reject`); open: `FileTracker` id-collision across projects untested
- [persistence](architecture/persistence.md) — flat-file-over-database convention across override_file, audit_log, FileTracker, and DaemonState; why rusqlite was removed unused; per-file corruption tolerance
- [observability](architecture/observability.md) — mobile/notification UX, the deferred-dashboard decision, CLI-first v1
- [trace-correlation](architecture/trace-correlation.md) — tagging harness dispatches with OTel resource attributes so tracker items link straight to their trace, posted as a structured `attach_link` attachment
- [error-stall-visibility](architecture/error-stall-visibility.md) — the most consistent gap across every system surveyed
- [ci-quality-tooling](architecture/ci-quality-tooling.md) — cargo-deny over cargo-audit, SHA-pinned actions, zizmor annotations over SARIF, weekly-only cargo-mutants

## Research (`research/`)

- [prior-art-landscape](research/prior-art-landscape.md) — the full systems-survey table
- [pm-layer-novelty](research/pm-layer-novelty.md) — verdict: proactive gap-detection is genuinely novel and undersolved — and why lucid still keeps it external rather than building it in
- [practitioner-reality](research/practitioner-reality.md) — what actually happens running these systems unattended
- [risks-and-critiques](research/risks-and-critiques.md) — the strongest skeptical arguments, with rebuttals
- [matplotlib-incident](research/matplotlib-incident.md) — the sharpest documented no-per-action-review failure
- [presence-automation-prior-art](research/presence-automation-prior-art.md) — idle/presence-gating outside coding agents
- [open-questions](research/open-questions.md) — what the research pass left unresolved
- [pi-harness-extensibility](research/pi-harness-extensibility.md) — pi.dev's minimal-core-plus-extensions philosophy compared against lucid's existing trait/profile-list seams

## Living outside the wiki

- `docs/FEATURES.md` § Deferred / not v1 — the current, authoritative open-items list. Intentionally left outside the wiki as a fast-moving checklist rather than a wiki page, since it changes shape faster than the wiki should churn. Re-check it each time the wiki is queried for "what's next."
