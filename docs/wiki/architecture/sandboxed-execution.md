# Sandboxed Execution: Where a Dispatch Actually Runs

Today, every dispatch — Worker, PM investigation, Agent reviewer — runs `claude -p` directly on the host machine via `tokio::process::Command`, inside a per-issue git worktree (`worktree::create`). The worktree is filesystem isolation for *which files get touched*; it is not process/kernel isolation. The only thing standing between an unattended `--permission-mode auto` Worker dispatch and the actual host is Claude Code's own permission classifier — there is no hard sandbox boundary underneath it.

That was an acceptable gap while every ticket originated from the operator themselves (self-authored, filed and approved by the same person running the daemon). It stops being acceptable once tickets can be externally triggered — [ticket ingestion via the tracker](human-in-the-loop.md) means a teammate's Discord message can become a `Pending` proposal without the operator writing a word of it. Approving that proposal still requires a human, but once approved, dispatch runs exactly as unsandboxed as anything else.

## What current practice says

Grounded via `research-first` (2026-08-19) against current writing on coding-agent sandboxing, specifically for the "let an agent run unattended with broad permissions" case lucid is already in: *"run your coding agents in a Docker sandbox... because the VM is the safety boundary, you can let agents run non-interactively without permission prompts"* (Shane Deconinck, 2026 — <https://shanedeconinck.be/posts/docker-sandbox-coding-agents/>). The framing matters: sandboxing isn't positioned as a *replacement* for a permission classifier, it's what makes granting broad, unattended permissions safe to do at all — the VM boundary is supposed to be doing the work `--permission-mode auto` alone is currently doing by itself in lucid.

The same source's comparison table: Docker-sandbox isolation is hypervisor-level with a private kernel ("Docker sandboxes restrict *where* the process exists"); native/host execution is OS-level restriction sharing the host kernel ("native sandboxing restricts *what* a process can do" — the category lucid's current `--permission-mode auto` falls into); devcontainers sit in between (namespace isolation, shared kernel).

## Naming convention: the unsafe option gets the scary name

Checked across every source in this research pass, including Claude Code's own precedent already in use in this codebase's own dispatch args: the permissive/unattended option always carries the explicit, cautionary name (`--dangerously-skip-permissions`), never the safe one. A CLI never ships `--safe-mode` as an opt-in; it ships the dangerous escape hatch as the thing you have to type out. This settles which way lucid's own default should point once a sandboxed backend exists: **sandboxed becomes the default, running on the bare host becomes the explicit, loudly-named opt-out** — not the reverse, and not a neutral-sounding `execution = "local"` config value that doesn't communicate the tradeoff.

## Design

`HarnessProfile` gains an execution axis, parallel to `kind`/`auth_mode`:

```rust
pub enum ExecutionBackend {
    Sandboxed, // default
    Local,     // explicit opt-in only
}
```

As shipped, `ExecutionBackend` stays `{ Sandboxed, Local }` — unit variants. The original design sketch (step 1 of this page) proposed `Sandboxed(SandboxKind)` once a real backend landed; that turned out unnecessary in practice — there's currently exactly one sandbox backend (Docker), selected by a single crate-level constant (`harness::SANDBOX_IMAGE`) rather than per-profile config, so there's nothing for a payload to discriminate between yet. If a second backend (e.g. a microVM) is ever added, that's the point to introduce the payload — not before.

`lucid.toml`'s `[[harness_profiles]]` entries default to requiring a sandbox; running one locally needs an explicit, unmistakably-named field — `unsandboxed = true`, not `execution = "local"` — so a config file reads as a decision, not a neutral setting someone toggled without noticing what it traded away. `HarnessProfile::validate()` enforces this: `execution_backend = Local` without `unsandboxed = true` fails config load outright (`Config::load` calls it for every profile), and `lucid config validate` additionally prints a `warning: profile \`{name}\` runs unsandboxed` line for every profile that does have the opt-out set, so an unsandboxed profile is visible even when the config is otherwise valid.

**Decided: self-hosted Docker (or microVM), not Claude Code's `--cloud` flag.** `--cloud` was a candidate raised mid-design (surfaced in `claude --help` output, never independently verified) but deliberately dropped — lucid stays harness-agnostic (Codex is a first-class profile kind, not a hypothetical), and `--cloud` would only ever cover the Claude Code half of that. Docker/microVM costs real infrastructure lucid now owns and operates, but it's the only candidate that works uniformly across every `HarnessKind`, which matters more than the zero-infra convenience `--cloud` would have offered for one harness only.

## Trust routing: a config-validated rail, not a convention

Any [project](multi-project.md) that accepts externally-triggered tickets must have a sandboxed harness profile configured — enforced by `lucid config validate` refusing to pass if a project's ticket sources include anything beyond the operator's own CLI/direct tracker approval, and no sandboxed profile exists for it. A convention that's only written down in a doc gets skipped under time pressure; a validator that refuses to start doesn't.

## Parallelism falls out of this, doesn't need its own design

`daemon.rs`'s current sequential-dispatch loop is sequential specifically because every dispatch today shares one host — one `worktree_root`, one process table. `Sandboxed` dispatches don't share that constraint: each gets its own isolated environment, so nothing prevents running several concurrently. `Local` dispatches keep the existing sequential behavior, since they still share the host. So "can this dispatch run in parallel with another" reduces to one already-modeled fact — its `ExecutionBackend` — rather than a separate scheduler concept to design. This also composes directly with [multi-project](multi-project.md): concurrent dispatch across *different* projects' sandboxed profiles is safe for the same reason concurrent dispatch within one project's sandboxed profile is.

## Build order

1. **Done.** `ExecutionBackend` enum + config shape (`unsandboxed = true` opt-out). `HarnessProfile::validate()` rejects `Local` without `unsandboxed = true`, and `lucid config validate` warns per unsandboxed profile.
2. **Done.** First real `Sandboxed` implementation: Docker. See "What's actually running" below for the shipped shape — `dispatch_with_fallback` (`src/harness/mod.rs`) now branches on `profile.execution_backend`: `Local` runs the same direct `tokio::process::Command` path as before (byte-identical env/arg construction, just refactored behind a `CommandSink` trait so it's shared with the sandboxed path rather than duplicated); `Sandboxed` builds a `docker run` invocation instead.
3. `lucid config validate` trust-routing rail (depends on [multi-project](multi-project.md)'s ticket-source concept existing first). Not built yet.
4. **Done.** Relax `daemon.rs`'s sequential loop for `Sandboxed` dispatches specifically. `daemon::dispatch_approved_issues` resolves one `ExecutionBackend` per dispatch batch (the lowest-`priority` configured profile's backend — `daemon::resolve_execution_backend`) and hands the batch to `daemon::dispatch_partitioned`: `Sandboxed` entries run concurrently with each other (`futures_util::future::join_all`), `Local` entries stay strictly sequential, and the two groups run concurrently with each other since neither shares a constraint the other needs to wait on. Concurrency within a `Sandboxed` batch is uncapped — every approved `Sandboxed` issue in a tick dispatches at once, one `docker run` each.

## What's actually running (step 2)

`docker/sandbox/Dockerfile` builds `lucid-sandbox:latest` (`node:22-slim` + `git` + both harness CLIs, `@anthropic-ai/claude-code` and `@openai/codex`, installed globally via npm — one image for every `HarnessKind`, since `profile.cmd` picks the binary at dispatch time and a single dispatch never needs to choose the image). Build it with:

```
docker build -t lucid-sandbox:latest -f docker/sandbox/Dockerfile .
```

For a `Sandboxed` profile, `dispatch_with_fallback` runs (roughly):

```
docker run --rm -i --network bridge -u <host-uid>:<host-gid> \
  -e <telemetry env vars> \
  -v <worktree-abs-path>:<worktree-abs-path> \
  -v <git-common-dir>:<git-common-dir> \
  -w <worktree-abs-path> \
  lucid-sandbox:latest <profile.cmd> <profile.args...> <dispatch flags> -- <prompt>
```

- **Mounts.** Only two bind mounts, both scoped to this one dispatch: the issue's worktree itself, and its git *common dir* (`git rev-parse --path-format=absolute --git-common-dir`, run against the worktree). The common-dir mount is a scoping compromise, not an oversight: a linked worktree's own `.git` is just a pointer file — `git commit` writes objects/refs into the *main* repo's `.git`, shared across every worktree of that repo. Mounting it is the only way `git commit` (which the dispatch prompt explicitly instructs the harness to run — see `worker::dispatch_prompt`) works inside the container at all. In practice this means the container can see *other* worktrees' git refs/objects (not their checked-out files, which live outside `.git` and aren't mounted) — a narrower exposure than "the whole host filesystem" but wider than "only this issue's worktree." No other paths are mounted; `-v` is the complete list, both bind mounts and nothing else — no host root, no docker.sock, no config dirs. Verified live (see below): a file outside these two mounts (`/tmp/.../host-secret.txt`) is unreadable from inside the container (`No such file or directory`), while `git commit` from inside the container against a real linked worktree lands correctly on the host, and files written by the dispatch land as the invoking user, not root.
- **Uid mapping.** `-u <host-uid>:<host-gid>` (via `id -u`/`id -g`, since this crate forbids `unsafe_code` so no `libc::getuid()`) — without it, `node:22-slim` runs as root by default and every file/commit the dispatch creates would come back root-owned on the host, breaking `worker.rs`'s post-dispatch `git log`/`git status` calls (which run as the daemon's own host user). `HOME=/tmp` is baked into the image since an arbitrary uid has no `/etc/passwd` entry, and some tools assume `$HOME` exists.
- **Network.** Docker's default `bridge` network — outbound NAT to the internet (so the harness's own model API calls work; DNS resolution and connectivity verified live against `api.anthropic.com`), isolated from the host's network namespace (no `localhost` access to whatever else is running on the operator's machine). No port publishing, no custom network.
- **Env vars.** The same `apply_telemetry`/`apply_dispatch_flags` logic as the `Local` path, but landing as `docker run -e KEY=VAL` flags before the image name instead of `Command::env` — env vars and the dispatched program's own trailing args sit on opposite sides of the image name in a `docker run` invocation, which is why `CommandSink` is a trait (`tokio::process::Command` for `Local`, a small `DockerArgs` buffer for `Sandboxed`) rather than the two paths sharing one `Command` object directly.

**Known gap, not yet designed:** subscription auth (`AuthMode::Subscription`) reads the harness CLI's own locally-stored login (e.g. `claude auth login`'s credential file on the host). A fresh `--rm` container has none of that — a `Sandboxed` profile with `auth_mode = Subscription` will fail to authenticate as shipped. Credential passthrough (mounting a read-only credential file, or provisioning per-dispatch short-lived tokens) is unsolved; today `Sandboxed` is proven out only for the isolation mechanism itself (verified live, see above), not yet exercised against a real authenticated `claude -p`/`codex exec` call. `examples/e2e_smoke.rs`, which does exercise a real authenticated subscription dispatch, deliberately stays on `Local`/`unsandboxed = true` for this reason.

Live-verification commands (not part of `cargo test` — see `examples/sandbox_livetest.rs`, itself an opt-in manual example following the same pattern as `e2e_smoke.rs`):

```
cargo run --release --example sandbox_livetest -- <worktree-dir> <host-only-file-path>
cargo run --release --example sandbox_livetest -- <worktree-dir> <host-only-file-path> git-commit
```
