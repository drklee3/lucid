# Pi's Extensibility Philosophy, and What It Suggests for lucid

Researched 2026-08-19 (`research-first`, standard tier — an architectural-philosophy question, not a version/API fact) against pi's own site and its author's writing, in response to a question about whether lucid's own modularity should look more like pi's.

## What pi actually does

[pi](https://pi.dev) (Mario Zechner / badlogic, `pi-mono` on GitHub, first released August 2025) is a terminal coding-agent harness built as a direct reaction to Claude Code's feature growth. Its own site states the thesis directly: "Pi is a minimal agent harness. Adapt Pi to your workflows, not the other way around." Source: [pi.dev](https://pi.dev).

Three concrete mechanisms carry that thesis:

- **A hard four-tool core** (`read`, `write`, `edit`, `bash`) — everything else is opt-in.
- **An explicit "Philosophy" exclusion list** — MCP, sub-agents, plan mode, permission popups, built-in TODOs, and background bash are all *named as deliberately absent*, each with a stated escape hatch (build it as an extension, or compose it from `bash`/`tmux`). Absence-as-a-documented-decision, not absence-as-a-gap — a reader can't mistake "not built yet" for "we chose not to."
- **A TypeScript extension API** as the one real extension point: extensions can register tools, inject messages before each turn, intercept/block/modify tool calls and their results, filter message history, and customize compaction. Skills (bundled instructions+tools, loaded on demand) and packages (extensions+skills+prompts+themes, installable via `npm`/`git`) sit on top of that same API. Source: [pi.dev](https://pi.dev).

The author's own rationale (Mario Zechner, ["What I learned building an opinionated and minimal coding agent"](https://mariozechner.at/posts/2025-11-30-pi-coding-agent/), 2025-11-30) names the actual failure modes this defends against, not just a minimalism aesthetic:

- **Context control** — feature-heavy harnesses "inject stuff behind your back that isn't even surfaced in the UI," which makes real context engineering impossible. Pi's answer is that nothing enters context the operator didn't explicitly wire in.
- **Observability** — built-in sub-agents in Claude Code are "black boxes within black boxes"; pi spawns sub-agents via `bash`/`tmux` instead, so they're just another visible, reviewable session, not a hidden abstraction.
- **Stability** — Claude Code's system prompt and tool set have changed under users mid-workflow ("frequent... changes... break workflows and change model behavior"); pi's minimal, rarely-touched core is a stability guarantee as much as a design taste.
- **Token cost only when used** — an MCP server like Playwright can dump ~13.7k tokens into context regardless of whether that turn needs it; file-based/CLI-composed tools give "progressive disclosure" instead — cost is paid only when the capability is actually invoked.
- **Externalize state, don't build it in** — no built-in plan mode or TODO tracking; instead the convention is a plain `PLAN.md`/`TODO.md` the agent reads and edits like any other file. Zechner's stated reasoning: built-in state tracking "confuse[s] models more than they help," and an external artifact is inspectable, diffable, and debuggable across sessions in a way in-process state isn't.

## Where lucid already matches this shape

lucid didn't copy pi, but three of its existing seams are structurally the same move — a small trait/interface plus a resolvable, prioritized list, instead of a single hardcoded implementation:

- `TrackerAdapter` (`src/tracker/mod.rs`) — Linear and file-backed trackers are two implementations of one trait; see [tracker adapter](../architecture/tracker-adapter.md).
- `PresenceSource` (`src/presence/mod.rs`) — pluggable idle-detection backends; see [presence detection](../architecture/presence-detection.md).
- The harness-profile list (`[[harness_profiles]]`, `ExecutionBackend`) — a prioritized, fallback-ordered list of (harness, auth mode, sandboxed/local) rather than one hardcoded `claude -p` call; see [harness dispatch](../architecture/harness-dispatch.md) and [sandboxed execution](../architecture/sandboxed-execution.md). [Harness dispatch](../architecture/harness-dispatch.md) itself already names this as "same list-with-fallback shape as the tracker adapter and presence sources, applied to harness+auth jointly" — the pattern is already recognized and reused deliberately, not accidental convergence.

This is the same underlying principle pi calls "adapt the tool to your workflow": a narrow trait boundary plus config-selected implementations, so adding a new tracker/presence-source/harness doesn't touch the daemon's core loop.

## Where the philosophies diverge, and why that's probably correct for lucid

Pi is a single-operator, interactive terminal tool optimizing for *one person's* context control and observability turn-by-turn. lucid is an unattended daemon whose job is dispatching to *other* harnesses (including `claude`/`codex` as opaque subprocesses) and reconciling tracker/PR state — it doesn't own a model's turn loop the way pi does, so pi's turn-level hooks (inject-before-turn, intercept-tool-call, custom compaction) don't have a lucid equivalent to attach to; the harness being dispatched already owns that loop. The applicable layer for lucid is one level up: the daemon's own control-flow seams (tracker, presence, harness selection, review/completion policy), not a model turn.

Two things are worth deliberately borrowing regardless of that difference:

1. **An explicit "Philosophy" / non-goals section**, naming what lucid deliberately doesn't do (e.g. no built-in chat UI, no in-process plan/TODO state beyond what the tracker item already holds, no MCP server of its own) so a reader can't read a gap in `docs/FEATURES.md` as "not gotten to yet" when it's actually "not the daemon's job." `docs/FEATURES.md` § Deferred already lists what's not built, but doesn't yet distinguish "deferred" from "deliberately out of scope" the way pi's Philosophy section does.
2. **Prefer externalized, plain-file state over new in-process abstractions when a plain artifact would do the same job** — this is already lucid's default (`state/tracker.json`, override files, audit log are all flat files — see [persistence](../architecture/persistence.md)), so no new work is implied here; it's confirmation the existing convention matches current best practice in this space, not a gap to close.

## Open question, not resolved here

Whether lucid should grow a real extension point of its own — e.g. a hook run before/after dispatch, or a pluggable `ReviewMode` beyond `Auto`/`Human`/`Agent` — wasn't asked and isn't answered by this research pass; it would need its own design page if pursued. This page documents the philosophy comparison only.

## Related pages

- [Tracker adapter](../architecture/tracker-adapter.md)
- [Presence detection](../architecture/presence-detection.md)
- [Harness dispatch](../architecture/harness-dispatch.md)
- [Sandboxed execution](../architecture/sandboxed-execution.md)
- [Persistence](../architecture/persistence.md)
- [Prior-art landscape](prior-art-landscape.md)
