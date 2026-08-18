# Features

Scoped units of work, organized by component. Traces to the decisions recorded in
`docs/wiki/architecture/` — nothing here should be new scope. Deferred items are
listed separately and are explicitly out of v1.

## Presence

- [x] `PresenceSource` trait: `is_idle(&self) -> bool`, `idle_since(&self) -> Option<Duration>`
- [x] Explicit override: a state file (default `$XDG_STATE_HOME/lucid/presence-override`) toggled by `lucid presence override`, always wins over automatic sources, no debounce (`src/presence/override_file.rs`)
- [ ] `logind` D-Bus source (`Lock`/`Unlock` signals, `IdleHint`/`IdleSinceHint`) via `zbus` — shape exists (`src/presence/logind.rs`), the actual D-Bus read is still `todo!()`; the daemon runs with an empty source list until this lands (conservative — see `main.rs::default_presence_sources`)
- [ ] Last-activity source: reads local agent session logs (Hermes, Claude Code, whatever's present) so an actively-driven interactive session blocks the autonomous flip even if the screen looks idle
- [x] Source composition: any source reporting "not idle" wins — conservative default, additive when more sources are added later
- [x] Debounce: idle must sustain past the full threshold (default 20 min), not just cross it, before flipping to autonomous (`presence::resolve`)
- [x] Mode-transition audit log — every flip is logged (trust-critical, needs an audit trail)

## PM / gap-detection

- [ ] Watermark record (repo file or tracker-side): last commit SHA reviewed, last wake timestamp, proposals-filed-this-week count — today's wake dispatches a fresh investigation each time, no incremental bounding yet
- [ ] Wake procedure's `git log <watermark>..HEAD` bounding, open-PR overlap check — not built; the prompt asks the harness to look at git history itself, unbounded
- [x] Gap-detection reasoning: dispatch to a configured harness, read-mostly and low-stakes by design (`src/pm.rs` — restricted `--allowedTools`, not the Worker's `auto` permission mode)
- [x] Proposal cap per wake cycle (`presence.proposal_cap_per_wake`, default suggested 3)
- [ ] Stub-only proposal format — title + "this goal seems to imply X isn't tracked yet, want to define it?", explicitly not a full spec — today's proposals are full `Proposal` structs, not gap-flag stubs
- [x] Dedup check before filing, via the tracker adapter's `query_similar` (`pm::wake`)

## Research agent

- [ ] Default tier (cheap): dependency/API existence check, in-repo prior-art grep, convention lint against CLAUDE.md/STYLE.md if present
- [ ] Deep tier (opt-in via PM tag on high-uncertainty/high-blast-radius proposals): throwaway-workspace prototype spike
- [ ] Confidence score returned alongside findings
- [ ] PM filing threshold: below it, discard rather than file, but still log to the rejected-ideas record so the same idea isn't re-investigated next wake

## Worker / dispatch

- [x] `HarnessProfile` type (name, kind, cmd, auth_mode, priority) and the ordered profile list (see `docs/wiki/architecture/harness-dispatch.md`)
- [x] Dispatch-with-fallback: try the highest-priority profile for the assigned harness; on a *detected block* (typed error signal parsed from `--output-format stream-json`'s `system/api_retry` events — `rate_limit`/`billing_error`/`oauth_org_not_allowed`-class, not any nonzero exit) fall through to the next profile
- [x] Hard per-dispatch timeout + `kill_on_drop` — a stalled harness process is killed and marked `TimedOut`, not implied by the design doc but added during live testing since an unbounded `cmd.output()` would otherwise hang the whole reconciliation tick forever
- [x] Per-issue git worktree isolation (`worktree::create`/`remove`) — every dispatch gets its own `lucid/<issue-id>` branch checked out under `daemon.worktree_root`, branched off `daemon.base_branch`'s tip, so `git add -A && git commit` can never scoop up an unrelated in-progress edit or another issue's uncommitted work. Matches current agent-orchestration practice's baseline safety rule ("never commit to a shared branch") flagged in `docs/wiki/architecture/worker-completion.md`. No path-prefix/hash-suffix collision handling beyond slugifying the issue id into the branch/dir name — not needed yet at today's sequential (one-issue-at-a-time) dispatch
- [ ] Lifecycle hooks: `after_create` / `before_run` / `after_run` / `before_remove`, only `before_run` failure aborts the run
- [x] Per-run phase state machine (Symphony's phases, plus the two states nothing surveyed had — awaiting-input, stuck/looping; see `src/state.rs`)
- [ ] Continuation-turn resume on review feedback — resume the same session, don't re-render a fresh prompt (fixes Symphony's known weak point)
- [x] Orchestrator-mediated tracker access only (see `docs/wiki/architecture/harness-tracker-isolation.md`) — the dispatched harness never gets a tracker credential or write tool
- [ ] Structured harness output (suggested comment / believed status / needs-input signal) parsed by the orchestrator — today's Worker posts a plain-text trace-link note (`worker::run_dispatch`), not a structured signal the reconciliation loop reads back. `ReviewMode::Agent`'s `VERDICT: PASS`/`FAIL` line (below) is a narrow structured signal for one specific decision, not a general replacement for this
- [x] Completion policy — every dispatch runs in its own worktree (above), commits its own work there under an appended prompt instruction (lucid never runs a git-mutating command itself, it lists every commit made via `git log <before>..HEAD`), then lucid pushes the branch and opens a PR via `gh pr create` (`pr::push_branch`/`pr::create`). `ReviewMode::Auto`/`Human`/`Agent` (who closes the tracker item) decides what happens to that PR: `Auto` and a `PASS`ed `Agent` review merge it immediately (`gh pr merge --squash --delete-branch`, `worker::mark_done`); `Human` and a failed `Agent` review leave it open for a human to merge from GitHub. A merge failure (conflict, unmet branch protection) is never auto-resolved — it routes to `NeedsReview` with `gh`'s own message attached. See `docs/wiki/architecture/worker-completion.md`
- [x] Deterministic verify gate for `ReviewMode::Agent` — lucid runs a verify command itself (`worker::run_verify_cmd`) and checks the real exit code before ever dispatching the LLM diff review, short-circuiting to `NeedsReview` on failure. The command resolves through two config tiers (`worker::resolve_verify_cmd`): a per-task `Proposal.verify_cmd` override, then a repo-wide `daemon.verify_cmd` default. Deliberately no auto-detection from repo shape (`Cargo.toml`/`package.json`, tried and removed) — a guessed command risks silently narrowing "verified" to less than the repo's real CI actually checks. Nothing set at either tier → the review agent infers its own command, same fallback as before this gate existed. Still doesn't verify the diff satisfies `acceptance_criteria` beyond "tests pass" — that judgment stays LLM-based. See `docs/wiki/architecture/worker-completion.md` § `verify_cmd`: deterministic, but deliberately optional — and resolved in two tiers

## Tracker adapter

- [x] `TrackerAdapter` trait: `create_proposal`, `set_decision_state`, `query_by_decision_state`, `query_similar`, `attach_note`
- [x] Linear implementation (GraphQL via `reqwest`, typed request/response structs via `serde` — see `docs/wiki/architecture/tracker-adapter.md` § `LinearAdapter` implementation notes)
- [x] File-backed local implementation (`src/tracker/file.rs`) — not in the original design, added so the dispatch/PM loop could be proven end-to-end without Linear credentials; a real backend, not a mock
- [x] Structured proposal issue body: title, one-line summary, "why now" bullets, effort estimate (S/M/L), risk note, YAML frontmatter (`task_type`, `target_paths`, `acceptance_criteria`, `research_ref`, `review`) — `render_description` (shared by both adapters) renders it, `TrackerIssue.description` carries it back out to the dispatch prompt, and `examples/e2e_smoke.rs` proved the full round trip live against a real `claude -p` dispatch
- [x] Decision surface: for `LinearAdapter`, moves the issue's real ticket `state` field (`Pending`/`Approved`/`In Review`/`Done`, four named workflow states required per team) via `issueUpdate`, not a label; `Rejected`/`StaleClosed` archive the issue (`issueArchive`) instead of using a fifth/sixth state — see `docs/wiki/architecture/tracker-adapter.md` § Decision state: the issue's real ticket state, not a label. `FileTracker` filters directly on its own `decision_state` field.
- [ ] Auto-stale-close after N days (default 7) — distinct from an explicit reject for dedup purposes
- [x] Dedup query: title-similarity match via `query_similar` (Linear's `searchIssues`; file backend's is a case-insensitive substring match) — content-hash matching and open-PR-file-overlap checking not built

## CLI / observability

- [x] `lucid start` — start the daemon (foreground only; detach/IPC not designed, see docs/CLI.md § Not yet designed)
- [ ] `lucid status` / `lucid show` — blocked on the same undesigned IPC; `Daemon::runs_snapshot` exists for an in-process caller but nothing cross-process reads it yet
- [x] `lucid presence status` / `lucid presence override` / `lucid pm wake` / `lucid config validate` / `lucid config show`
- [x] `lucid task list` / `approve` / `reject` / `dispatch-now` — a terminal convenience over the tracker's own UI, not a second source of truth: every subcommand is a thin call through the same `TrackerAdapter` (`query_by_decision_state`/`set_decision_state`) the daemon itself uses. `dispatch-now` runs the daemon's exact dispatch path (`worker::dispatch_and_finalize`, now shared between `daemon.rs` and the CLI) on demand for one already-`Approved` issue — it changes *when* approved work runs, never *whether* (see `docs/wiki/architecture/worker-completion.md`)
- [x] `lucid task create` — files a new `Proposal` via `TrackerAdapter::create_proposal` directly from the CLI, printing the new issue id. The real write path for `Proposal.review`/`Proposal.verify_cmd`: previously the only ways to set either were hand-editing a ticket's frontmatter text or constructing a `Proposal` struct in Rust (`examples/e2e_smoke.rs`). Skips `pm::wake`'s `query_similar` dedup check by design — a human typing a title explicitly isn't the re-filing case dedup guards against.
- [x] Full command tree, flags, and output formats: see `docs/CLI.md`

## Reconciliation loop

- [x] Poll tick: presence-gated dispatch of `Approved`-decision-state issues, retrying a previous `Failed`/`TimedOut` run (`src/daemon.rs`) — deliberately **sequential per tick, not concurrent** (a real task-supervisor with cross-tick polling was judged more machinery than this pass warranted; see the module doc on `Daemon`), so stall protection is per-dispatch-timeout rather than a separate concurrent stall detector
- [ ] Refresh tracker state for all claimed issues each tick: terminal → cleanup, no-longer-active → stop without cleanup, still-active → update snapshot — today only checks "is this issue still in the `Approved` decision state," doesn't detect a human un-approving/closing an in-flight issue mid-run
- [x] Parked-state rule for a *completed* run: `ReviewMode::Human`/`Agent`-on-fail move the issue to `DecisionState::NeedsReview`, out of the `Approved` decision state entirely, so the dispatch loop never re-picks it up until a human moves it (see `docs/wiki/architecture/worker-completion.md`). Distinct from parking a *stuck/looping in-flight* run — `WorkerPhase::AwaitingHumanInput` still exists but nothing produces it yet
- [x] Every tick, every `NeedsReview` issue's PR is checked (`gh pr view --json state` via `pr::status`) and reconciled to the human's already-made decision: merged → `Done`, closed without merging → `Rejected`, still open (or no PR found yet) → left alone. Runs on every tick regardless of presence mode — it only records a decision a human already made, never dispatches anything
- [ ] Persist reconciliation/session state to `rusqlite` so a restart doesn't lose in-flight tracking — still in-memory only (`Daemon.runs`), explicitly not yet fixing the Symphony in-memory-only weakness the design called out

---

## Deferred / not v1

Explicitly out of scope for the first build — not forgotten, just sequenced later:

- Web dashboard (CLI-only for v1; becomes a real ask only if CLI check-ins become the bottleneck)
- GitHub Issues tracker adapter (Linear first; the interface should make this a second implementation, not a rewrite, once it's actually needed)
- Non-`logind` presence sources (Windows-host `GetLastInputInfo` for WSL2, macOS IOKit, etc.) — the trait supports them, but none are built until a specific environment needs one
- Review/rework auto-trigger policy — auto-resume-on-any-comment (cyrus-style) vs. explicit-mention-required (Copilot-style) is a real, deliberately undecided fork in the road (see `docs/wiki/architecture/review-rework-ux.md`)
- Loop/unproductive-progress ("stuck") *detection logic* — the state exists in the state machine, but no heuristic for actually detecting it is designed yet
- Merge-conflict handling — flagged as unsolved industry-wide in the gap analysis, no design answer yet
- Proactive stall notification (active push instead of passive dashboard/log checking) — named as a plausible differentiator, not designed
- "Proof of work" artifacts attached to the tracker item/PR (CI status, walkthrough video, etc.) — noted pattern from Symphony/Cursor; one concrete instance (trace-correlation links) is now built and live-verified (spans actually land in the trace backend, correlated by `dispatch_id` — see `docs/wiki/architecture/trace-correlation.md`) — the rest (CI status, walkthrough video) still undesigned
- Runaway/self-replicating-session guard — noted gap from cyrus's issue tracker, not designed
- Rate-limit-specific failure handling as a distinct class from generic retry — noted gap, not designed
