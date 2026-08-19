# Sandboxed Execution: Where a Dispatch Actually Runs

Today, every dispatch — Worker, PM investigation, Agent reviewer — runs `claude -p` directly on the host machine via `tokio::process::Command`, inside a per-issue git worktree (`worktree::create`). The worktree is filesystem isolation for *which files get touched*; it is not process/kernel isolation. The only thing standing between an unattended `--permission-mode auto` Worker dispatch and the actual host is Claude Code's own permission classifier — there is no hard sandbox boundary underneath it.

That was an acceptable gap while every ticket originated from the operator themselves (self-authored, filed and approved by the same person running the daemon). It stops being acceptable once tickets can be externally triggered — [ticket ingestion via the tracker](human-in-the-loop.md) means a teammate's Discord message can become a `Pending` proposal without the operator writing a word of it. Approving that proposal still requires a human, but once approved, dispatch runs exactly as unsandboxed as anything else.

## What current practice says

Grounded via `research-first` (2026-08-19) against current writing on coding-agent sandboxing, specifically for the "let an agent run unattended with broad permissions" case lucid is already in: *"run your coding agents in a Docker sandbox... because the VM is the safety boundary, you can let agents run non-interactively without permission prompts"* (Shane Deconinck, 2026 — <https://shanedeconinck.be/posts/docker-sandbox-coding-agents/>). The framing matters: sandboxing isn't positioned as a *replacement* for a permission classifier, it's what makes granting broad, unattended permissions safe to do at all — the VM boundary is supposed to be doing the work `--permission-mode auto` alone is currently doing by itself in lucid.

The same source's comparison table: Docker-sandbox isolation is hypervisor-level with a private kernel ("Docker sandboxes restrict *where* the process exists"); native/host execution is OS-level restriction sharing the host kernel ("native sandboxing restricts *what* a process can do" — the category lucid's current `--permission-mode auto` falls into); devcontainers sit in between (namespace isolation, shared kernel).

## Naming convention: the unsafe option gets the scary name

Checked across every source in this research pass, including Claude Code's own precedent already in use in this codebase's own dispatch args: the permissive/unattended option always carries the explicit, cautionary name (`--dangerously-skip-permissions`), never the safe one. A CLI never ships `--safe-mode` as an opt-in; it ships the dangerous escape hatch as the thing you have to type out. This settles which way lucid's own default should point once a sandboxed backend exists: **sandboxed becomes the default, running on the bare host becomes the explicit, loudly-named opt-out** — not the reverse, and not a neutral-sounding `execution = "local"` config value that doesn't communicate the tradeoff.

## Design

`HarnessProfile` gains an execution axis, parallel to `kind`/`auth_mode` (same shape, not a new abstraction):

```rust
pub enum ExecutionBackend {
    Sandboxed(SandboxKind), // default
    Local,                  // explicit opt-in only
}
```

As shipped (build order step 1, `src/harness/mod.rs`), `ExecutionBackend` is `{ Sandboxed, Local }` — unit variants, no `SandboxKind` payload. There is no real sandboxed backend yet (step 2), so there's nothing for a payload to name; the tuple form above is the target shape once a Docker/microVM backend exists, not the current one.

`lucid.toml`'s `[[harness_profiles]]` entries default to requiring a sandbox; running one locally needs an explicit, unmistakably-named field — `unsandboxed = true`, not `execution = "local"` — so a config file reads as a decision, not a neutral setting someone toggled without noticing what it traded away. `HarnessProfile::validate()` enforces this: `execution_backend = Local` without `unsandboxed = true` fails config load outright (`Config::load` calls it for every profile), and `lucid config validate` additionally prints a `warning: profile \`{name}\` runs unsandboxed` line for every profile that does have the opt-out set, so an unsandboxed profile is visible even when the config is otherwise valid.

**Decided: self-hosted Docker (or microVM), not Claude Code's `--cloud` flag.** `--cloud` was a candidate raised mid-design (surfaced in `claude --help` output, never independently verified) but deliberately dropped — lucid stays harness-agnostic (Codex is a first-class profile kind, not a hypothetical), and `--cloud` would only ever cover the Claude Code half of that. Docker/microVM costs real infrastructure lucid now owns and operates, but it's the only candidate that works uniformly across every `HarnessKind`, which matters more than the zero-infra convenience `--cloud` would have offered for one harness only.

## Trust routing: a config-validated rail, not a convention

Any [project](multi-project.md) that accepts externally-triggered tickets must have a sandboxed harness profile configured — enforced by `lucid config validate` refusing to pass if a project's ticket sources include anything beyond the operator's own CLI/direct tracker approval, and no sandboxed profile exists for it. A convention that's only written down in a doc gets skipped under time pressure; a validator that refuses to start doesn't.

## Parallelism falls out of this, doesn't need its own design

`daemon.rs`'s current sequential-dispatch loop is sequential specifically because every dispatch today shares one host — one `worktree_root`, one process table. `Sandboxed` dispatches don't share that constraint: each gets its own isolated environment, so nothing prevents running several concurrently. `Local` dispatches keep the existing sequential behavior, since they still share the host. So "can this dispatch run in parallel with another" reduces to one already-modeled fact — its `ExecutionBackend` — rather than a separate scheduler concept to design. This also composes directly with [multi-project](multi-project.md): concurrent dispatch across *different* projects' sandboxed profiles is safe for the same reason concurrent dispatch within one project's sandboxed profile is.

## Build order

1. **Done.** `ExecutionBackend` enum + config shape (`unsandboxed = true` opt-out), `Local` behaves identically to today — no behavior change until a `Sandboxed` backend actually exists. Shipped as unit variants (`Sandboxed`, `Local`), config-surface only: `HarnessProfile::validate()` rejects `Local` without `unsandboxed = true`, and `lucid config validate` warns per unsandboxed profile. `dispatch_with_fallback` and all dispatch behavior are unchanged.
2. First real `Sandboxed` implementation: Docker/microVM. Once this exists, `ExecutionBackend::Sandboxed` gains its `SandboxKind` payload.
3. `lucid config validate` trust-routing rail (depends on [multi-project](multi-project.md)'s ticket-source concept existing first).
4. Relax `daemon.rs`'s sequential loop for `Sandboxed` dispatches specifically.

Each step is gated on the one before it; only step 1 is built so far.
