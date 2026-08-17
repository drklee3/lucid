# Features

Scoped units of work, organized by component. Traces to `docs/design.md`'s resolved
decisions — nothing here should be new scope. Deferred items are listed separately
and are explicitly out of v1.

## Presence

- [ ] `PresenceSource` trait: `is_idle(&self) -> bool`, `idle_since(&self) -> Option<Duration>`
- [ ] Explicit override: a state file (`state/mode`) toggled by an explicit command, always wins over automatic sources, no debounce
- [ ] `logind` D-Bus source (`Lock`/`Unlock` signals, `IdleHint`/`IdleSinceHint`) via `zbus` — reference implementation
- [ ] Last-activity source: reads local agent session logs (Hermes, Claude Code, whatever's present) so an actively-driven interactive session blocks the autonomous flip even if the screen looks idle
- [ ] Source composition: any source reporting "not idle" wins — conservative default, additive when more sources are added later
- [ ] Debounce: idle must sustain past the full threshold (default 20 min), not just cross it, before flipping to autonomous
- [ ] Mode-transition audit log — every flip is logged (trust-critical, needs an audit trail)

## PM / gap-detection

- [ ] Watermark record (repo file or tracker-side): last commit SHA reviewed, last wake timestamp, proposals-filed-this-week count
- [ ] Wake procedure: `git log <watermark>..HEAD` (bounded, not full history), open tracker issues (for dedup), open PRs (avoid colliding with in-flight work), wiki/ROADMAP read for direction
- [ ] Gap-detection reasoning: dispatch to a configured harness, read-mostly and low-stakes by design
- [ ] Proposal cap per wake cycle (default 3)
- [ ] Stub-only proposal format — title + "this goal seems to imply X isn't tracked yet, want to define it?", explicitly not a full spec
- [ ] Dedup check before filing, via the tracker adapter's `query_similar`/`query_by_label`

## Research agent

- [ ] Default tier (cheap): dependency/API existence check, in-repo prior-art grep, convention lint against CLAUDE.md/STYLE.md if present
- [ ] Deep tier (opt-in via PM tag on high-uncertainty/high-blast-radius proposals): throwaway-workspace prototype spike
- [ ] Confidence score returned alongside findings
- [ ] PM filing threshold: below it, discard rather than file, but still log to the rejected-ideas record so the same idea isn't re-investigated next wake

## Worker / dispatch

- [ ] `HarnessProfile` type (name, cmd, auth_mode, priority) and the ordered profile list from decision #8
- [ ] Dispatch-with-fallback: try the highest-priority profile for the assigned harness; on a *detected block* (typed error signal — `rate_limit`/`billing_error`/`oauth_org_not_allowed`-class, not any nonzero exit) fall through to the next profile
- [ ] Per-issue git worktree isolation: path-prefix checks against workspace root, sanitized workspace keys with hash-suffix on collision
- [ ] Lifecycle hooks: `after_create` / `before_run` / `after_run` / `before_remove`, only `before_run` failure aborts the run
- [ ] Per-run phase state machine (Symphony's phases, plus the two states nothing surveyed had — awaiting-input, stuck/looping; see `src/state.rs`)
- [ ] Continuation-turn resume on review feedback — resume the same session, don't re-render a fresh prompt (fixes Symphony's known weak point)
- [ ] Orchestrator-mediated tracker access only (decision #7) — the dispatched harness never gets a tracker credential or write tool
- [ ] Structured harness output (suggested comment / believed status / needs-input signal) parsed by the orchestrator, which is the only thing that ever calls the tracker adapter's write methods

## Tracker adapter

- [ ] `TrackerAdapter` trait: `create_proposal`, `set_decision_state`, `query_by_label`, `query_similar`
- [ ] Linear implementation (GraphQL via `reqwest`, typed request/response structs via `serde`)
- [ ] Structured proposal issue body: title, one-line summary, 2-3 "why now" bullets, effort estimate (S/M/L), risk note, YAML frontmatter (`task_type`, `target_paths`, `acceptance_criteria`, `research_ref`)
- [ ] Decision surface: label/state transition (`proposal:pending` → `proposal:approved`/`proposal:rejected`)
- [ ] Auto-stale-close after N days (default 7) — distinct from an explicit reject for dedup purposes
- [ ] Dedup query: content-hash/title-similarity match, rejected-label check, open-PR-file-overlap check (decision #6) — queried live, no local mirror

## CLI / observability

- [ ] `lucid start` — start the daemon
- [ ] `lucid status` — list running/blocked/retrying agents (Symphony's dashboard content model, as CLI output)
- [ ] Full command tree, flags, and output formats: see `docs/CLI.md`

## Reconciliation loop

- [ ] Poll tick: stall-detect running work (`elapsed_ms > stall_timeout_ms` since last event)
- [ ] Refresh tracker state for all claimed issues each tick: terminal → cleanup, no-longer-active → stop without cleanup, still-active → update snapshot
- [ ] Parked-state rule, made explicit (not just implied): once a Worker's item is in a human-review-equivalent state, stop dispatching/polling it entirely until a human moves it
- [ ] Persist reconciliation/session state to `rusqlite` so a restart doesn't lose in-flight tracking — explicitly not repeating Symphony's in-memory-only blocked-state weakness

---

## Deferred / not v1

Explicitly out of scope for the first build, per design.md — not forgotten, just sequenced later:

- Web dashboard (CLI-only for v1; becomes a real ask only if CLI check-ins become the bottleneck)
- GitHub Issues tracker adapter (Linear first; the interface should make this a second implementation, not a rewrite, once it's actually needed)
- Non-`logind` presence sources (Windows-host `GetLastInputInfo` for WSL2, macOS IOKit, etc.) — the trait supports them, but none are built until a specific environment needs one
- Review/rework auto-trigger policy — auto-resume-on-any-comment (cyrus-style) vs. explicit-mention-required (Copilot-style) is flagged in design.md as a real, deliberately undecided fork in the road
- Loop/unproductive-progress ("stuck") *detection logic* — the state exists in the state machine, but no heuristic for actually detecting it is designed yet
- Merge-conflict handling — flagged as unsolved industry-wide in the gap analysis, no design answer yet
- Proactive stall notification (active push instead of passive dashboard/log checking) — named as a plausible differentiator, not designed
- "Proof of work" artifacts attached to the tracker item/PR (CI status, walkthrough video, etc.) — noted pattern from Symphony/Cursor, not designed
- Runaway/self-replicating-session guard — noted gap from cyrus's issue tracker, not designed
- Rate-limit-specific failure handling as a distinct class from generic retry — noted gap, not designed
