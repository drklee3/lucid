# Autonomous Agentic Development System — Brainstorming & Research

## System Concept

A multi-agent orchestration system where agents don't just execute tasks but **proactively investigate**, **propose work**, and **continue development** when I'm away from keyboard. It's composed of open-source building blocks — agnostic to tracker, coding harness, and models.

### Core Roles

1. **PM Agent** — Investigates repos, identifies direction/goals, proposes tasks rooted in best practices and research. Hands off to research agents for validation. Files proposals as tracker issues. This is the *novel* piece — nothing existing covers this.

2. **Research Agent** — Validates proposed tasks: feasibility checks, best practices lookup, dependency compatibility, prior art. Returns findings; PM decides to file or discard.

3. **Worker Agent** — Monitors tracker for approved tasks, executes via coding harness (Claude Code / Codex / etc.), opens PRs. This half exists in many forms already.

4. **Human** — Reviews proposals in tracker as binary decisions (👍/👎 per issue, not drowning in context). Reviews PRs. The system should respect presence: when I'm at the keyboard, I drive; when idle, it continues autonomously.

### Scope Clarification: Gap-Detection, Not Open-Ended Ideation

The PM agent does not decide overall direction. Direction (goals, roadmap) stays a human call — the same three-category split the research survey found in the market (triage existing tickets / decompose a human-supplied spec / invent an idea from nothing) is real, and full autonomy over "what to build" is the one nobody has shipped and the one Linear explicitly declined to build. This system does not aim for that category either.

What it aims for instead: given a goal or direction the human has already stated (a wiki page, a roadmap doc, a stated priority), the PM agent notices when a **concrete gap exists between that stated goal and the current ticket/PR set** — something the goal implies but nothing yet tracks — and files a *stub*, not a full proposal: title + "this goal seems to imply nothing addresses X yet, want to define it?" It doesn't spec the work; it flags the gap and hands it to the human to define scope, same as answering "what's next?" when asked interactively, just running unprompted on a schedule instead of on-demand.

This narrower framing matters for two reasons found in the research:
- It avoids the failure mode behind the matplotlib incident (an agent with a stake in its own idea reacting badly to rejection) — a gap-flag has no ego in the outcome, rejection just means the human already knows or disagrees the gap matters.
- It keeps the review cost small. The "review burden shifts, doesn't disappear" critique applies to full proposals; a one-line gap-flag is cheap to glance at and dismiss, which is a materially smaller ask than what that critique is about.

This only works if there's a goal artifact concrete enough for "gap" to be well-defined — not a vague vibe. Reinforces why PM investigation scope (#2 below) needs a real, reasonably current wiki/ROADMAP to diff against, not just git log.

### Key Constraints

- **Tracker-agnostic** — Not locked to Linear. Swap via MCP adapter (Linear, GitHub Issues, GitLab, Plane).
- **Harness-agnostic** — Not locked to Codex. Worker runs `claude -p`, `codex`, `aider`, or whatever via CLI command.
- **Model-agnostic** — Each agent role could use different models (cheap for research, expensive for implementation).
- **Presence-aware trigger** — Not a naive cron. Detects idle/screen-lock to transition into autonomous mode. No death loops from push events.

## Existing Systems Research

### What exists (none cover the PM/proposal layer)

**OpenAI Symphony** — SPEC.md + Elixir reference implementation. Polls Linear, spawns per-issue Codex agents in isolated workspaces. Key findings:
- **Not a product** — OpenAI explicitly won't maintain it. Reference implementation only.
- **Codex-locked** — JSON-RPC app-server protocol. Claude Code doesn't speak it.
- **Linear-locked** — State machine maps to Linear's status model. Porting to GitHub Issues requires rebuilding the adapter layer.
- **No PM layer** — Reactive dispatch of existing tickets only. No "investigate and propose" capability.
- **Community forks exist** — Multiple ports to Claude Code (TypeScript, Go, Rust), but all take 1-4 weeks and hit the same integration bugs (CLI parsing, GraphQL quirks, PTY requirements).
- **Universal praise**: WORKFLOW.md pattern (repo-owned agent prompts, versioned with code), per-issue workspace isolation, reconcile-before-dispatch loop.
- **Universal problem**: Review/rework loop is broken — agents treat feedback as total reset, not incremental fix.

**Linear Coding Sessions** (June 2026) — Linear Agent codes via Claude Code/Codex in cloud sandboxes. Triage automations auto-investigate bugs (~30% resolved first pass). Closed ecosystem.

**Factory Missions** — Multi-day orchestration with orchestrator → workers → validators. Orchestrator plans and decomposes but never codes. Workers implement with fresh contexts. Validators verify independently. TDD at two levels. Proprietary.

**MetaSwarm, Maestro, SWE-AF, Miyabi, AGYN** — Various open-source multi-agent frameworks. All cover the worker/dispatch half. None cover the proactive PM/proposal layer.

### Key architectural takeaways from research

1. **Orchestrator never codes** — Every working system separates planning from implementation.
2. **Fresh contexts per sub-task** — No giant sessions. Work is decomposed into focused units.
3. **Independent validation** — Implementer never validates its own work.
4. **WORKFLOW.md pattern** — Repo-owned agent contract versioned with code. Universal praise.
5. **Daemon model > CI-triggered** — Long-lived service with retries, reconciliation, stall detection.

## Proposed Architecture

**Architectural correction (supersedes the diagram below in spirit, kept for history):** the original sketch made Hermes the host process. Revised: the orchestration core is a deterministic control loop (poll, dispatch, track state, retry, reconcile) — a systems-engineering problem, not an LLM-reasoning one, and doesn't need or benefit from running inside an agent framework. It's its own small standalone process. Hermes becomes one interchangeable *harness backend* (`hermes -p "task"`, on equal footing with `claude -p` / `codex`), not the foundation everything else sits on — this is actually what "harness-agnostic" (a Key Constraint since the top of this doc) already implied; embedding in Hermes would have quietly violated it. See "High-Level Components & Tech Stack" below for the corrected version.

```
Hermes (orchestrator — already running)
  ├── PM bot profile ── reads repo, proposes tasks → writes to tracker
  ├── Research skill ── validates proposals (web search, best practices)
  ├── Worker executor ── runs `claude -p` in isolated workspaces
  └── Presence hook ── checks idle → triggers PM mode

Tracker ── MCP server (swapable: Linear, GitHub Issues, Plane)
Coding harness ── CLI command (swapable: claude, codex, aider)
```

The **PM agent** is the novel, high-differentiation piece. The worker dispatch is plumbing that exists.

### Open Design Questions

1. **Presence detection** — How to transition between active and autonomous mode? xprintidle? Screen lock events? Last-message timestamp? Explicit command?

2. **PM investigation scope** — What does the PM agent actually read when it wakes up? Full repo? Recent git log? Open issues? Linear backlog? Does it maintain a ROADMAP.md or equivalent?

3. **Binary decision format** — How do proposals land in the tracker as quick yes/no reads? Structured issue format with clear 3-line summary + decision field?

4. **Agent handoff surface** — How does the PM ensure the worker interprets the proposal correctly? Structured output schema in the tracker issue?

5. **Research depth** — How much validation is enough before filing a proposal? Shallow (check deps exist)? Deep (prototype feasibility)?

6. **Death loop prevention** — What prevents PM from proposing the same task repeatedly? Dedup by issue content? Block on open PRs?

## Symphony SPEC.md — patterns worth stealing directly

Read `openai/symphony/SPEC.md`. Concrete mechanisms worth reusing verbatim in this system:

- **Orchestration state separate from tracker state.** Symphony tracks `Unclaimed → Claimed (Running|RetryQueued) → Released` internally, independent of whatever the tracker calls its statuses. Adopt this: the standalone orchestrator owns the true state machine, the tracker adapter is just a projection. This is what makes tracker-swapping tractable — the adapter maps *our* states to *its* labels, not the other way around.
- **Per-run phase tracking.** `PreparingWorkspace → BuildingPrompt → LaunchingAgentProcess → InitializingSession → StreamingTurn → Finishing → {Succeeded|Failed|TimedOut|Stalled|CanceledByReconciliation}`. Worth copying for the Worker executor's own logging/observability, even if we don't need all eleven phases.
- **Continuation turns, not context resets.** "The first turn SHOULD use the full rendered task prompt. Continuation turns SHOULD send only continuation guidance to the existing thread." This is the fix for Symphony's own known weak point (review/rework loop treated as a full reset). When a human leaves PR feedback, the Worker should resume the *same* session (`claude -p --resume <session-id>` or equivalent) with just the review comments as the new turn — not re-render the whole issue as a fresh prompt. Bake this into WORKER.md as a hard requirement, not an optimization.
- **WORKFLOW.md frontmatter contract.** YAML frontmatter (tracker kind, active/terminal states, polling interval, `agent.max_turns`) + Markdown prompt body, strict template engine, fails loud on unknown variables. Steal this shape for our own PM.md / RESEARCH.md / WORKER.md role contracts — repo-owned, versioned with code, fails loud instead of silently misrendering.
- **Workspace isolation invariants.** Per-issue workspace path, must stay inside workspace root (prefix-check on normalized absolute paths), sanitized workspace key with a hash suffix on collision. `after_create`/`before_run`/`after_run`/`before_remove` hooks, with only `before_run` failures aborting the run. Directly reusable for Worker's isolated checkouts.
- **Reconciliation tick.** Every tick: (1) stall-detect running agents (`elapsed_ms > stall_timeout_ms` since last event → kill + retry), (2) refresh tracker state for all claimed issues (terminal → cleanup, no-longer-active → stop without cleanup, still-active → update snapshot). This is the daemon-model backbone — adopt wholesale for the Worker executor loop.

## Resolved design decisions (supersedes "Open Design Questions" below)

**1. Presence detection is a pluggable source list, not a single mechanism.** Same shape as the tracker adapter and the harness/auth profile list (#3, #8): the orchestrator depends on a small internal interface (`is_idle() -> bool`, `idle_since() -> Option<Duration>`), and any number of concrete sources can implement it — `logind` D-Bus is the first/reference implementation, not the only one that will ever exist. Sources compose (any source reporting "not idle" wins — conservative default), so adding or swapping a source later is additive, not a rewrite. Layer three *kinds* of signal, most authoritative first:
   - Explicit override: a state file (owned by the standalone orchestrator, e.g. its own `state/mode`) toggled by an explicit command — always wins, no debounce.
   - One or more automatic idle sources — `logind` D-Bus (`Lock`/`Unlock` signals + `IdleHint`/`IdleSinceHint`) is the reference implementation; other sources (a Windows-host `GetLastInputInfo` read for WSL2, a macOS IOKit reader, etc.) plug into the same interface later, only when actually needed for whatever environment the orchestrator runs on next.
   - Last-activity timestamp from *any* known local agent session log (Hermes, Claude Code, etc. — whatever's actually running on this machine) — if the user was actively driving any session in the last N minutes, don't flip to autonomous even if the screen looks idle — e.g., they're reading a long tool output. This is a read, not a dependency — the orchestrator doesn't need to be hosted by whichever tool it's reading from.
   Debounce the transition itself (require idle sustained for the full threshold, not just crossed it) to avoid flapping when the user steps away briefly. Log every mode transition — this is a trust-critical piece of the system and needs an audit trail.

   **Verified on this machine (WSL2), logged for later, not urgent now:** `systemd-logind` is running and reachable on the system D-Bus (`org.freedesktop.login1`, PID present, `busctl` confirms the service) — so the reference implementation is buildable here. But its idle tracking is dead in this specific environment — `loginctl list-sessions` shows the active session's `IdleHint=yes` stuck for over a week, because WSL2 has no real "seat" with physical input devices behind it (`CanGraphical=no`, `seat0` has no sessions attached) — nothing inside the WSL2 VM generates HID activity events into logind. This doesn't block building the `logind` source now (it's still the correct reference implementation, and will work as-is on a real Linux desktop later); it just means presence-gated autonomy won't actually trigger correctly *on this machine* until a second source (last-activity-log, or eventually a Windows-side input reader) is added to the list — a known gap to close before actually flipping the system into autonomous mode here, not before starting to build it.

**2. PM investigation scope.** Give the PM a repo-owned watermark file, `docs/wiki/PM_STATE.md` or a tracker-side equivalent, recording: last commit SHA reviewed, timestamp of last wake, count of proposals filed this week. On wake: `git log <watermark>..HEAD` (not full history — bounds the read), open tracker issues (dedup input, see #6), open PRs (don't propose work colliding with in-flight PRs), and the wiki/ROADMAP if one exists (direction, not just diff). Cap proposals per wake (recommend 3) — a PM that files 15 issues at 3am is worse than useless, it's a wall the human has to triage past to find the good ones.

**3. Binary decision format.** **Today's tracker backend: Linear**, chosen for v1 rather than deferred — Linear's own remote MCP server (`mcp.linear.app`, OAuth2.1, full issue/project/comment CRUD) is directly usable by any MCP-capable client, not something that requires Hermes as an intermediary (Hermes just happens to have this pre-wired, which is a nice-to-have shortcut during prototyping, not a dependency of the design). Reused because Linear satisfies three requirements at once that a local-only store can't: single source of truth, remote/mobile management via Linear's native app, and reuse of a mature product instead of building triage UI from scratch. Critically, this stays swappable: PM/Worker code talks to a thin internal tracker-adapter interface (`create_proposal`, `set_decision_state`, `query_by_label`, `query_similar`), never to Linear-specific API/label concepts directly — same principle as the Symphony "orchestration state separate from tracker state" pattern noted above. A GitHub Issues (or other) adapter is a second implementation of that interface later, not a rewrite. Structured issue body: title, one-line summary, 2-3 bullet "why now," effort estimate (S/M/L), risk note, and a machine-readable YAML frontmatter block (see #4). Decision surfaces via the adapter as whatever binary affordance the backend supports (Linear: label/state, e.g. `proposal:pending` → `proposal:approved` / `proposal:rejected`) — Linear's mobile app renders that as a tappable action for free. No reaction after N days (recommend 7) auto-closes as stale, distinct from an explicit reject (matters for #6's dedup memory).

**4. Agent handoff surface.** Same frontmatter-body split as WORKFLOW.md, embedded in the issue itself: `task_type`, `target_paths`, `acceptance_criteria` (list), `research_ref` (link to the Research agent's findings, not re-summarized prose). The Worker parses this deterministically instead of inferring intent from freeform issue text — this is the single biggest lever for avoiding the "worker misinterprets vague proposal" failure mode.

**5. Research depth.** Tiered, not fixed:
   - Default (cheap): dependency/API existence check, grep for prior art in-repo, lint against repo conventions (CLAUDE.md, STYLE.md if present). One shallow pass, cheap model.
   - Deep (expensive, opt-in by PM tag): a throwaway-workspace prototype spike, only for proposals the PM itself flags as high-uncertainty or high-blast-radius (schema/migration changes, anything security-sensitive, anything touching auth).
   Research agent returns a confidence score alongside findings; PM has a filing threshold below which it discards rather than files (logged to the rejected-ideas list either way, so it doesn't re-investigate the same idea next wake).

**6. Death loop prevention.** Before filing, PM checks three things: (a) open Linear issues with matching content hash/title similarity (queried live via MCP, not a local mirror), (b) issues carrying the rejected label (👎'd or auto-stale-closed within N days — recommend 30), (c) open PRs touching the same files. Any hit blocks filing. Linear itself is the source of truth for this check — no separate local dedup store to keep in sync or lose. This is the single most important piece of state in the whole system — losing it silently reintroduces every idea a human already said no to.

**7. No direct tracker access for dispatched harnesses.** Neither the Worker's coding harness (`claude -p` / `codex` / `hermes -p`, whichever) nor the Research agent gets the Linear MCP (or any tracker credential) wired in directly. All tracker interaction — reads and writes alike — is mediated by the orchestrator through the adapter interface from #3:
   - **Reads**: the orchestrator resolves relevant context (related open issues, prior comments, PM gap-detection findings, watermark state) via the adapter *before* dispatch and injects it into the prompt as plain text/structured summary. A harness never does its own live Linear query. If a long-running session needs fresher context mid-task, that's what the continuation-turn mechanism (resolved decisions, Symphony pattern) is for — the orchestrator pushes updated context on resume, the harness doesn't pull it.
   - **Writes**: a harness never gets a "post comment" or "change status" tool. It produces structured output as part of its turn — a suggested comment, a status it believes is warranted, a "needs human input" signal — and the *orchestrator* is the only thing that ever calls `set_decision_state` or posts a comment through the adapter.
   This is a safety property, not just cleanliness: it's the concrete mitigation against the matplotlib-incident failure mode (an agent with a lever over the outcome of its own review) — no harness, on any tracker, ever has live write access to its own tracker item. It's also a direct win for harness-agnosticism: none of `claude -p`/`codex`/`hermes -p` need Linear (or any tracker) configured at all, so nothing tracker-specific needs to stay in sync across harnesses.

**8. Harness dispatch is a prioritized list of (harness, auth mode) profiles, subscription-first.** Both Claude Code and Codex CLI support the same dual-auth pattern in headless mode: a subscription login (Claude Code: plain `claude -p`, reads existing OAuth same as interactive; Codex: `codex exec`, reads a ChatGPT Plus/Pro/Business login) works with no API key, and each flips to metered API billing the moment the corresponding env var is set (`ANTHROPIC_API_KEY` for Claude Code — though note this specifically requires `--bare`, which itself refuses to read OAuth credentials at all; `OPENAI_API_KEY` for Codex, which auto-overrides a logged-in session). Given both are already-paid-for subscriptions, default to subscription auth and treat API-key billing as the fallback, not the default — inverting the earlier draft of this decision, which defaulted to `--bare`+API-key preemptively. Concretely, harness dispatch is a small ordered list of profiles, not a single hardcoded command:
   ```
   [ {name: "claude-subscription", cmd: "claude -p ...",        auth: subscription, priority: 1},
     {name: "claude-api",          cmd: "claude --bare -p ...", auth: ANTHROPIC_API_KEY, priority: 2},
     {name: "codex-subscription",  cmd: "codex exec ...",       auth: subscription, priority: 1},
     {name: "codex-api",           cmd: "codex exec ...",       auth: OPENAI_API_KEY, priority: 2} ]
   ```
   The orchestrator tries the assigned harness's subscription profile first; on a detected block (not any nonzero exit — a specific signal: Claude Code's stream carries typed retry-error categories like `rate_limit`/`billing_error`/`oauth_org_not_allowed` in its `system/api_retry` events; Codex has an analogous error surface) it falls through to the next profile in priority order — same harness's API-key variant, or a different harness entirely. This is the same list-with-fallback shape already used for the tracker adapter, applied to harness+auth jointly rather than treating "which harness" and "which auth" as separate axes.
   - **ToS note, resolved not just assumed:** the restriction found during research (`"Anthropic does not allow third party developers to offer claude.ai login... for their products, including agents built on the Claude Agent SDK"`) is about products serving *other* users through your infrastructure — it does not apply here. This system uses the actual `claude` CLI binary, logged in as the account owner, driving that owner's own subscription for personal automation — not the Agent SDK library, and not a product offered to third parties. Compliant, not a gray area.
   - **Real tradeoff, not a compliance one: shared capacity.** Subscription usage draws from the same rolling 5-hour/weekly window as interactive daytime use — an autonomous Worker running overnight competes with tomorrow's interactive session for the same pool. API-key billing is metered but doesn't compete at all. This is the actual reason to keep the fallback live, not a hedge against a ToS problem.
   - **Forward-looking, not urgent:** Anthropic has stated `--bare` (API-key-only) "will become the default for `-p` in a future release" — not yet, but the fallback-list design means that flip is a priority reordering, not a rewrite, when it happens.

## High-Level Components & Tech Stack

**Revised after questioning whether Hermes should be the host at all** (see "Architectural correction" note above) — it shouldn't. The deterministic orchestration core is its own small standalone process/service, not embedded in Hermes. Hermes is one interchangeable harness backend among several (`claude -p` / `codex` / `hermes -p`), same footing as any other. Confirmed decision: Worker isolation is the orchestrator's own simple git-worktree management (Symphony's approach), not a dependency on Hermes's sandbox machinery — simpler, and keeps the one component that dispatches *to* Hermes from also being *hosted by* Hermes, which would have been circular.

### Implementation language: Rust

**Decision, superseding an earlier wrong turn.** The first pass at this reasoned "presence detection needs D-Bus, Python's D-Bus bindings are more mature, therefore Python" — that's a scoping error: presence detection is one narrow subsystem (a background watcher reading a couple of `systemd-logind` properties over D-Bus), not something that should determine the language for the whole system, and the maturity claim itself doesn't hold up on inspection — `zbus` is a mature, actively-maintained, async-native Rust D-Bus crate, arguably more modern than the Python options it was being compared against. Once the two questions are separated and checked honestly, Rust is the better fit on the merits, independent of preference:

- **It's a long-running system daemon** — a compiled binary with no runtime/interpreter dependency is a better fit for a `systemd --user` service than an interpreted-language process: faster startup, lower idle memory footprint, nothing extra to keep on `PATH`.
- **State machine correctness.** This system is, at its core, several explicit state machines (Worker phases, tracker decision state, presence mode) — Rust's enums carry data per variant (not C-style tags) and `match` is compiler-enforced exhaustive: adding a new state and forgetting to handle it somewhere is a compile error, not a live bug. This directly targets a failure pattern that showed up repeatedly in the gap-analysis research — OpenHands' `ERROR` state marked "optional for future use" in its own source, cyrus's cluster of silent-failure issues from states not being fully surfaced, Symphony's blocked-state map not persisting across restarts. All three are "a state existed but wasn't fully handled" bugs, which is exactly the class Rust's exhaustiveness checking catches at compile time. This matters more here than in a typical CRUD app, since our design already extends Symphony's state machine with states nothing we surveyed had (an explicit "awaiting human input" state, a "stuck/looping" state distinct from stall-timeout) — more custom states than any single system we looked at, which is exactly where an unenforced state machine tends to grow gaps.
- **Concurrency without a new runtime paradigm.** The daemon tracks multiple in-flight Worker sessions concurrently (Symphony's own reason for existing — supervising many agents at once without babysitting each by hand). `tokio` async tasks plus Rust's ownership model give the same class of safety Symphony gets from Elixir/BEAM's actor model, without introducing an unfamiliar runtime.
- **CLI**: `clap` — excellent, arguably best-in-class across any language.
- **Linear GraphQL**: `reqwest` + `serde` with typed structs for the specific queries/mutations behind the tracker-adapter interface — a real advantage over Python's dict-shaped JSON for a deterministic backend, not just parity.
- **Local state store**: `rusqlite` for the daemon's own bookkeeping (running/blocked/retry state, session identity for continuation-turn resume) — specifically so a restart doesn't lose in-flight state the way Symphony's does.
- **Git worktree + harness dispatch**: `std::process::Command` / `tokio::process` shelling out to `git` and to whichever harness CLI — no library needed, same approach Symphony and cyrus both take regardless of their own implementation language.
- **D-Bus/presence**: `zbus`, async-native, no disadvantage versus any other language's bindings for this.

What's still genuinely reusable, reframed as *optional conveniences* rather than dependencies: Linear's MCP server is Linear's own product, callable by any MCP-capable client — nothing about using it requires routing through Hermes. Hermes-the-harness is invoked the same way `claude -p` or `codex` would be: a CLI subprocess call, nothing more.

```
Standalone orchestrator (its own small always-on process or scheduled service —
language/runtime TBD, doesn't need to be Python/Hermes-specific; a systemd
timer + script or a lightweight always-on loop both work)
  │
  ├── Presence watcher            [NET NEW]
  │     → logind D-Bus idle-hint watcher (resolved decision #1); writes its
  │       own state, gates whether the PM/Worker loops are allowed to act
  │
  ├── PM gap-detection job        [NET NEW]
  │     → runs on the orchestrator's own scheduling loop (poll tick,
  │       Symphony-style reconciliation) — no dependency on any particular
  │       host's cron system
  │     → reads: watermark file, goal/wiki doc, git log, open Linear issues
  │       (via the tracker adapter, live query)
  │     → emits: gap-flag stubs → Linear issues via the tracker adapter,
  │       labeled `proposal:pending`
  │     → the actual reasoning step (gap-detection logic itself) is a call
  │       out to an LLM — could be Claude Code, Codex, or Hermes, whichever
  │       is configured; this is the one place where "which harness" matters
  │       least, since it's read-mostly and low-stakes by design (#3 stub-only
  │       scope)
  │
  ├── Research agent               [NET NEW]
  │     → same pattern: orchestrator invokes a harness (any of them) scoped
  │       to a research prompt, gets back findings + confidence score
  │
  ├── Tracker adapter              [NET NEW, small — Linear impl first]
  │     → orchestrator logic talks to a thin internal interface
  │       (create_proposal, set_decision_state, query_by_label,
  │       query_similar), never to Linear-specific concepts directly — the
  │       Symphony "orchestration state separate from tracker state" pattern
  │     → today's implementation: Linear, via its own remote MCP server
  │       (mcp.linear.app, OAuth2.1) — any MCP client library works, this
  │       doesn't require Hermes's MCP wiring specifically, though reusing
  │       Hermes's already-authenticated connection is a fine shortcut for
  │       prototyping if convenient
  │     → a GitHub Issues adapter is the natural second implementation once
  │       the interface is proven out against real Linear use
  │     → the pending-cap + dedup-latching-on-dismiss pattern (originally
  │       spotted in Hermes's cron/suggestions.py) is worth keeping as a
  │       design reference even though it's not being reused as code — the
  │       orchestrator re-implements the same shape against Linear as the
  │       state store (see resolved decisions #3 and #6)
  │
  ├── Worker executor              [NET NEW, deliberately simple]
  │     → owns its own per-issue git worktree isolation (Symphony's
  │       workspace invariants: path-prefix checks, sanitized keys,
  │       after_create/before_run/after_run/before_remove hooks) — no
  │       dependency on Hermes's 7-backend sandbox machinery
  │     → harness dispatch: a CLI subprocess call, swappable —
  │       `claude -p`, `codex`, `hermes -p`, whatever's configured for this
  │       task, all on equal footing
  │     → continuation-turn handling (resolved decision, Symphony's known
  │       weak point fixed): resume the same session on review feedback
  │       rather than re-render a fresh prompt — mechanism depends on
  │       whichever harness's session-resume support (`claude -p --resume`,
  │       equivalent for others)
  │
  └── Presence/scheduling backbone [NET NEW, small]
        → Symphony's reconciliation tick (stall-detect running work, refresh
        tracker state, requeue/cleanup) is the reference model — a poll
        loop, not a framework; genuinely simple to implement standalone
```

### What this means concretely

- **This is a small standalone service, not a Hermes feature.** The earlier framing ("extend what Hermes already has") was a wrong turn — it made a deterministic control loop dependent on an LLM agent framework's process model for no real benefit, and would have quietly broken the harness-agnostic constraint from day one by making Hermes load-bearing instead of interchangeable.
- **Almost everything here is net-new**, and that's fine — it's all small, well-scoped pieces (a poll loop, a D-Bus watcher, a 4-method tracker adapter, git-worktree management, subprocess dispatch), not a framework to build. Symphony proves this shape works as a lean standalone daemon; there's no reason ours needs to be bigger.
- **Linear's MCP and Hermes's existing OAuth wiring are conveniences for prototyping, not dependencies of the design.** Worth using what's already authenticated and working while building v1, but the tracker-adapter boundary means nothing breaks if that changes later.
- **The PM's actual differentiated work is still the gap-detection *logic*, not infrastructure** — diffing a stated goal doc against the current ticket/PR/git-log state and deciding something concrete is missing. That's a prompt/reasoning problem invoked *by* the orchestrator, not something the orchestrator itself needs to be smart about.

## UX / State-Machine Gap Analysis (grounded via /research-first against Symphony, Linear Coding Sessions, cyrus, GitHub Copilot coding agent, Factory Missions, OpenHands, Cursor)

Gap list only — no redesign yet. Each item names what an existing system does that our current design (sections above) doesn't yet address, and why it might matter.

### State machine

- **No "awaiting human input" state, distinct from blocked/stalled.** Linear's Agent Sessions has a dedicated `awaitingInput` status driven by an `elicitation` activity type — the agent explicitly pauses mid-task to ask a clarifying question, and a reply resumes it via a `prompted` webhook. Our design's phase list (borrowed from Symphony) has no equivalent — a Worker that needs to ask something has nowhere to go but stall-detection or a generic error. This is a real gap if we want the Worker to ask a scoping question rather than guess and ship wrong.
- **No loop/unproductive-progress detection, only elapsed-time stall detection.** OpenHands has a first-class `STUCK` terminal state (distinct from generic `ERROR`, which its own source marks "optional for future use" — i.e. even OpenHands hasn't fully solved this). Symphony's `Stalled` is purely `elapsed_ms > stall_timeout_ms` — silence, not unproductive activity. Earlier research (the "while loop" HN thread) found agents that don't go silent, they just loop on low-value busywork (rewriting tests, endless TODO.md updates) — a time-based stall timer never fires for that. We have no design answer for this failure mode at all.
- **Symphony's "parked state stops polling" pattern is good and should be made explicit for us, not just implied.** Once an issue is in `Human Review` (outside Symphony's `active_states`), Symphony stops dispatching/polling it entirely until a human moves it — cheap and correct. Our doc implies something similar via decision-state gating but never states it as a rule for the Worker's own state machine, only for the PM's proposal flow.

### Review/rework UX — a real fork in the road, not yet decided

- **cyrus and GitHub Copilot coding agent made opposite defaults, both deliberately.** cyrus auto-resumes on *any* PR "Request Changes" review (webhook on `pull_request_review`, no mention needed) and on any Linear thread reply. Copilot explicitly does **not** act on native review comments — only on an explicit `@copilot` mention — a documented, deliberate safety change (their changelog frames it as preventing the agent from "interpreting notes as commands"). Our resolved decisions say "continuation turns, not resets" (correct, and better than Symphony's own reset-on-Rework flow) but never picked a side on *auto-resume vs. explicit-trigger*. Given the matplotlib incident already logged in our research, this is worth deciding deliberately rather than defaulting to whichever is easier to build.
- **cyrus is also the cautionary tale for how to build the trigger.** Its webhook-driven continuation is the most automatic of anything surveyed, but its own open issue tracker documents session-identity bugs adjacent to this exact mechanism (Cursor has the same failure independently: GitHub-issue-triggered follow-ups sometimes spin up a *new* session instead of resuming the old one, breaking the whole point of continuation). Whatever trigger we build needs an explicit, tested session-identity key, not an assumption that "webhook fires → resume the right thread" just works.
- **No system reviewed has solved merge-conflict handling.** Copilot explicitly does not auto-rebase; its "Fix with Copilot" one-click resolver is unreliable per multiple 2026 community threads. Symphony's WORKFLOW.md sidesteps the problem rather than solving it — `Rework` does a hard reset (close PR, fresh branch off `origin/main`) specifically so it never has to rebase an old branch. Our "continuation turns" decision is better for context/review economy but doesn't have Symphony's out — if a continuation-based Worker's branch goes stale relative to `main`, we don't yet have an answer, and neither does anyone else surveyed. Worth flagging as unsolved-industry-wide rather than assuming our approach will just handle it.

### Mobile/notification UX

- ~~The Linear-mobile assumption needs a narrower check~~ — **resolved, not a gap.** PR review happens natively in GitHub (GitHub has its own mature mobile review flow — confirmed above in the Copilot research: filter/sort sessions, review, merge-conflict fixes from GitHub Mobile), not in Linear. Linear's role is scoped to the proposal/gap-flag layer only (issue-level label toggle), which its mobile app already handles fine. This cleanly separates concerns: Linear = task/proposal tracking, GitHub = code review — no cross-tool mobile gap to chase.
- **Push notification granularity is undocumented on Linear's side.** We assumed "Linear mobile push notifications" solve the remote-visibility requirement, but Linear's own docs don't confirm distinct push events for "agent done" vs "agent stuck" vs "PR ready" — only a generic "you'll be notified when input is needed" claim and one changelog bugfix referencing a "completion notification." Cursor's native iOS app is more explicit here (documented push on "work ready for review"). Worth a direct check before assuming Linear's mobile notifications give us the "stuck at 3am" signal we actually want.
- **Factory's "shareable session link, zero-install, watch or take over" pattern has no equivalent in our design** — not necessarily needed for a single-user system, but worth naming as a deliberately-skipped feature rather than an unnoticed gap.

### Dashboard/observability

- **We currently have no dashboard plan at all** — only per-run phase logging (borrowed from Symphony) with no viewer. Every system surveyed except cyrus (self-hosted) has *something*: Symphony's optional LiveView dashboard (Blocked/Retrying/Running tables, color-coded badges, token/runtime metrics — though blocked-state is in-memory only and lost on restart, a limitation worth not repeating), Copilot's session-log transcript viewer with an `Agent-Logs-Url` commit trailer for audit, OpenHands' full live IDE+terminal+browser view (the most immersive of anything surveyed), Cursor's real-time progress + diff viewer + attached videos/screenshots/logs on the PR itself so a reviewer doesn't have to re-run locally.
- **Concrete, low-cost fix available for later:** Hermes already has a webui subsystem running (`~/.hermes/webui`, `webui.log` observed). Symphony's dashboard content model (Blocked table with last-error, Retry Queue table, Running table, color-coded state badges) is a fully specified, proven-rough draft — adopting that shape as a Hermes webui page is cheap relative to designing one from scratch, whenever it's built.
- **Decision: web dashboard deferred, not needed for v1.** Important eventually, not now. v1 observability is CLI-only, modeled on the same content Symphony's dashboard shows (running/blocked/retrying agents) but rendered as terminal output, not a web page: one command starts the daemon (`hermes pm start` or equivalent), a second lists/inspects active agents (`hermes pm status` / `ps`-for-agents — Symphony's Blocked/Retrying/Running table content, as CLI output). Linear serves as the async, periodic check-in surface (proposal review, "what's next" gap-flags) rather than live monitoring — live monitoring is what the CLI is for. The web dashboard becomes a real ask once/if CLI-checking in becomes the bottleneck, not before.
- **"Proof of work" artifacts attached to the tracker item** (Symphony's stated goal: "CI status, PR review feedback, complexity analysis, and walkthrough videos" so a human can approve without re-running anything; Cursor attaches videos/screenshots/logs to the PR) is a pattern we haven't considered. Worth adding to the Worker's PR-completion behavior, not just the diff — this stays relevant even with the dashboard deferred, since it's about what lands on the GitHub PR / Linear issue itself, not a dashboard feature.

### Error/stall visibility — the most consistent gap across every system

- **cyrus's live GitHub issue tracker is the single most concrete evidence found in either research pass**, more valuable than any marketing page: silent Linear API rate-limit stalls that break in-flight sessions with no surfacing (#1324), a completed session's result silently discarded after an interrupt (#1389), an infinite retry loop burning API quota with no visible signal (#1378), runaway self-replicating sessions (#1328), unbounded memory growth from uncleaned completed sessions leading to OOM (#1381), and a cloud job that stayed visible but never executed with zero progress comment (#1250). This confirms, in a live tracker rather than an anecdote, exactly the "silent/latent failure is the dominant failure mode" finding from the earlier practitioner-reality research.
- **None of our resolved decisions currently address**: rate-limit-specific handling as a distinct failure class (vs. generic retry), a runaway/self-replicating-session guard, bounded cleanup of completed session state, or persistence of blocked/error state across a Hermes restart (Symphony's own dashboard state doesn't survive a restart either — worth not inheriting that specific weakness).
- **No system surveyed has solved proactive stall notification** — every one of them relies on the human noticing or polling a dashboard/log, not on an active push when something goes quiet. Given we're already building on Linear (mobile push) specifically to solve the remote-visibility requirement, closing this gap — an active "Worker stalled, here's why" push rather than passive dashboard-checking — is a plausible place to actually do better than every system surveyed, not just match them.

## Next steps for this Claude Code session

1. Pick the standalone orchestrator's own runtime/language and process model (systemd timer + script vs. a lightweight always-on loop) — this is now an open decision that didn't exist under the old "runs inside Hermes" framing.
2. Draft `PM.md`, `RESEARCH.md`, `WORKER.md` role-contract files using the WORKFLOW.md frontmatter+body shape — the concrete artifact the orchestrator loads and renders per-harness, regardless of which harness (`claude -p` / `codex` / `hermes -p`) ends up dispatched.
3. Prototype the `logind` D-Bus idle-hint watcher as a standalone script (Python + `dbus-idle`, or raw `python-dbus`) and confirm it fires correctly under the actual compositor in use here.
4. Pick the first tracker adapter to build (Linear vs GitHub Issues) and sketch its state-mapping table (our internal states ↔ its labels/statuses) per the Symphony pattern in the first section above.
5. Design the rejected-ideas/dedup store concretely — flat file vs tracker-native query vs small local DB — before writing any PM logic that depends on it.
6. Decide the review/rework trigger explicitly (auto-resume-on-any-review-comment, cyrus-style, vs. explicit-mention-required, Copilot-style) — this is a real fork in the road surfaced by the gap analysis above, not a detail to leave implicit.
7. Design the two v1 CLI commands (start-daemon, list/inspect active agents) against Symphony's Blocked/Retrying/Running table content as the reference for what "inspect" should show — this is the actual v1 observability surface, not a placeholder for a future dashboard.
8. Prototype the Worker's git-worktree isolation directly against Symphony's invariants (path-prefix checks, sanitized keys, lifecycle hooks) — this is now net-new code to write, not something inherited from Hermes.