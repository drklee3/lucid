# Human-in-the-Loop: Ingestion, Notification, and Mid-Task Input

Design for three things asked together, because they're one workflow: file tickets from anywhere, get notified when lucid needs a decision, and let an in-flight Worker ask a scoping question without ever blocking a process on the answer. Resolves three previously-open gaps: [state-machine-gaps](state-machine-gaps.md)'s missing `AwaitingHumanInput` producer, `docs/FEATURES.md`'s unbuilt lifecycle hooks, and [symphony-patterns](symphony-patterns.md)'s "continuation turns, not context resets" requirement.

## Ticket ingestion: no new lucid code

Don't build an ingestion path into lucid. Linear (the tracker) is already the multi-source intake point — a Discord bot, a Slack bot, an email parser, anything, can create a Linear issue directly via Linear's own API/integrations, tagged however makes sense on that side. `daemon::dispatch_approved_issues` already treats "anything in `Approved`" uniformly regardless of what created it or how — it's already source-agnostic. The only actual gap is the *outbound* side: nothing tells a human when lucid needs them.

## Notification: a `NotificationSink` trait, not a Discord-specific integration

New trait, same shape as `TrackerAdapter` — pluggable, one method per lifecycle event that matters to a human:

```rust
#[async_trait]
pub trait NotificationSink: Send + Sync {
    async fn on_awaiting_input(&self, issue: &TrackerIssue, question: &str) -> anyhow::Result<()>;
    async fn on_needs_review(&self, issue: &TrackerIssue, pr_url: Option<&str>) -> anyhow::Result<()>;
    async fn on_done(&self, issue: &TrackerIssue) -> anyhow::Result<()>;
}
```

Each call site already exists and already knows exactly what it needs to say — `worker::finalize_completion`'s `NeedsHuman` branch, the new needs-input branch (below), and `mark_done`'s success path. A no-op `NullSink` is the default (matches today's behavior exactly); a `WebhookSink` (templated: `{issue_title}`, `{issue_url}`, `{pr_url}`, `{question}` interpolated into a user-configured message template, POSTed to any URL) is the first real implementation — covers Discord, Slack, or anything else that takes a webhook, without lucid knowing anything Discord-specific. `lucid.toml` gets one new optional `[notifications]` section: a template string per event and a webhook URL. No hardcoded Discord dependency in the binary at all.

## The needs-input mechanism: end-of-turn signal, never a live pause

Confirmed directly against current Claude Code docs (`research-first` pass, 2026-08-18): there is no headless "ask and wait" primitive. `AskUserQuestion` is explicitly denied outside interactive/`dontAsk`-exempt modes, and a `-p` session that needs input has exactly one option — end its turn and exit. This is a hard constraint, not a missing feature to work around differently: **the mechanism cannot be a blocking wait inside a running process**, because `-p` doesn't support one and `daemon.stall_timeout_secs` would kill it anyway (correctly — a process blocked for hours would be indistinguishable from a hung one).

So: the dispatch prompt gains an instruction — "if a decision only a human can make blocks you (ambiguous scope, a choice with real tradeoffs, missing access), end your turn instead of guessing: `NEEDS_INPUT: <question>`." The harness process exits normally, the same way it already does today for every other outcome. Chosen over `--json-schema` for now: `--json-schema` requires `--output-format json`, not `stream-json` — and lucid's block-detection (`system/api_retry` parsing) and session-id capture depend on `stream-json`'s incremental event stream. Whether `--json-schema` composes with `stream-json` at all is unverified in the docs (its examples are all single-shot extraction, not a long agentic tool-use session like Worker/Reviewer dispatches). The marker-line approach needs no verification because it's already proven in this exact codebase: `pm::wake`'s `extract_json_array` already tolerantly extracts structured content from free-text model output today, in production. Revisit `--json-schema` once its `stream-json` compatibility is confirmed — it would be strictly more robust than string matching.

## State wiring

`WorkerPhase::AwaitingHumanInput` (already declared, never produced) gets its producer: `run_dispatch` (or a thin wrapper around it) checks the result text for a `NEEDS_INPUT:` prefix before falling through to the existing success/failure classification. On a match:

1. `WorkerPhase::AwaitingHumanInput`, `DecisionState` stays `Approved` — **not** `NeedsReview**. It needs its own state, or `dispatch_approved_issues`'s retry check (`Failed`/`TimedOut` only) needs to explicitly skip re-dispatching an `AwaitingHumanInput` run, the same way it already skips `Succeeded`. Simplest: treat it exactly like `NeedsReview` today — moved out of `Approved` into a state the dispatch loop ignores — but tag *which* issues are parked-for-input vs. parked-for-review, since they resolve differently.
2. **The worktree is kept alive**, not torn down — the one place `dispatch_and_finalize`'s "always remove the worktree" rule gets an explicit exception. The question might reference specific file state the human wants to look at, and resuming needs a real checkout to run in.
3. `notification_sink.on_awaiting_input(issue, question)` fires.
4. `tracker.attach_note` posts the question to the issue too (Linear-visible even without the notification channel, same as every other outcome already does).

## Resolving it: reuse `Approved`, reuse the persisted session_id

Your own instinct was right — reuse `TrackerAdapter` state-watching rather than inventing a reply channel. Concretely: the human answers **as a Linear comment**, then moves the issue back to `Approved` (via `lucid task approve` or Linear directly) — the exact same "human says go" signal that already exists for every other flow, not a new one. No new detection code, no new CLI command, no new webhook-receiving surface for lucid to run.

The part that's actually new: when `dispatch_approved_issues` picks the issue back up, it must **not** build a fresh `dispatch_prompt` — that would lose all the context from the paused session, exactly the "context reset" [symphony-patterns](symphony-patterns.md) already named as the failure mode to avoid. Instead: `DaemonState.runs` (persisted to disk since [worker-completion](worker-completion.md)'s state-persistence work) already has this issue's `WorkerRun.session_id` from the paused run. If it's present and the run's last phase was `AwaitingHumanInput`, dispatch via `claude -p --resume <session_id> "<human's latest comment text>"` in the *same* kept-alive worktree, instead of `dispatch_prompt(issue)` in a fresh one. Confirmed compatible: `--resume` with `-p` is documented and works cross-directory since v2.1.223 — though keeping the same worktree sidesteps needing that cross-directory behavior at all.

## Never hangs forever

Nothing is waiting in a process, so "hangs forever" can only mean "sits parked indefinitely" — a data problem, not a process problem, and it has the same shape as the already-flagged "auto-stale-close after N days" idea. `reconcile_needs_review`'s tick step (or a sibling to it) gets a companion check: an `AwaitingHumanInput` issue whose question was posted more than `daemon.awaiting_input_timeout` ago (config, default something like 3 days) gets a note attached ("no response after N days") and moves to `NeedsReview` — falls back to a human noticing it in their normal review queue instead of a specialized reminder system, and stops it from parking silently forever if the notification itself was missed or ignored.

## Open finding, adjacent to this design: dispatches aren't isolated from the operator's own Claude Code environment

Not part of this feature, but surfaced while grounding it: every lucid dispatch today runs plain `claude -p ...` with no `--bare` — meaning it loads the *operator's* full `~/.claude/settings.json`, hooks, skills, and MCP servers, not an isolated environment. Directly observed: a manual reproduction this session showed `SessionStart` hooks firing (a `honcho` hook erroring, others) — noise from the operator's personal config bleeding into what's supposed to be an unattended dispatch. Current Claude Code docs call `--bare` "the recommended mode for scripted and SDK calls," soon to become `-p`'s default.

Not a free fix, though: `--bare` also skips the project's own `CLAUDE.md` — which has been doing real, wanted work all session (commit-message conventions, unprompted wiki updates matching the repo's own documented rules). The right shape is `--bare` **plus** explicitly injecting the repo's `CLAUDE.md` via `--append-system-prompt-file`, rather than relying on auto-discovery of *whichever* machine happens to run the daemon. This is also the session-level analog of the isolation already discussed for PR authorship (a dedicated bot account, not the operator's own `gh` identity) — worktree isolates the filesystem, a dedicated `gh` identity isolates authorization, `--bare` + explicit `CLAUDE.md` injection isolates the Claude Code session itself from whatever happens to be configured on the machine running the daemon. Worth its own task, independent of everything else on this page.

## Build order

Each piece below is independently shippable:

1. `--bare` + explicit `CLAUDE.md` injection for all dispatches (Worker, PM, Reviewer) — smallest, no design risk, matches documented current best practice.
2. `NotificationSink` trait + `NullSink` default + `WebhookSink` (templated) — additive, zero behavior change until configured.
3. `NEEDS_INPUT:` marker parsing + `AwaitingHumanInput` producer + worktree-keep-alive exception + `on_awaiting_input` hook.
4. Resume-on-reapproval: check `DaemonState.runs` for a stored `session_id` before building a fresh prompt; dispatch via `--resume` when present.
5. Staleness timeout for parked `AwaitingHumanInput` issues, alongside the existing `reconcile_needs_review` tick step.

(4) depends on (3); everything else is independent and can land in any order.
