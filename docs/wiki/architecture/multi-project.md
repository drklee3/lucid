# Multi-Project: One Daemon Instance, Many Repos

Today `lucid.toml` and `Daemon` both assume exactly one repo, one tracker binding, one `workdir`. This page designs the shape for one running daemon instance to manage several repos at once, rather than one `lucid start` process per repo.

## What Symphony already does here

[Symphony patterns](symphony-patterns.md) already documents the relevant precedent: `WORKFLOW.md`, a **repo-owned agent contract, checked into and versioned with each repo's own code** — tracker binding, polling interval, workspace/hook config, all declared by the repo itself, not centralized in the orchestrator's own config. Multi-repo support in Symphony isn't "one config listing N repos"; it's "N repos, each self-declaring how it wants to be worked on."

`lucid.toml` today is the opposite of that: `/lucid.toml` is gitignored, treated as local runtime state alongside `.env`, not a versioned contract. That's a real inconsistency with a pattern the wiki's own research already flagged as worth adopting ("WORKFLOW.md pattern — repo-owned agent contract versioned with code; universal praise across systems surveyed" — [symphony-patterns.md](symphony-patterns.md)).

## Split: global daemon config vs. per-project, repo-owned config

**Stays global** (one instance, shared across every project): presence detection, observability/OTel config, tick interval, stall timeout. There's exactly one human whose "away" state matters — it doesn't vary by which repo is being worked on, so presence gating stays a single top-level concern, not something threaded per-project.

**Becomes per-project**: tracker binding (team/project/backend), `workdir`, `base_branch`, `worktree_root`, `verify_cmd`, the PM-wake backoff timer, and dispatch retry-tracking (`runs`).

Config shape: the central `lucid.toml` gains a `[[projects]]` array (same pattern `[[harness_profiles]]` already uses) — each entry a **pointer**, not a full config: `path = "/home/drk/github/repo-a"`. Each pointed-to repo carries its own lightweight checked-in config file for the repo-specific bits (tracker project key, `verify_cmd`, `base_branch`) — Symphony's `WORKFLOW.md` shape, adapted. The daemon's own config says *which repos to watch*; each repo's own file says *how it wants to be worked on*. A project's settings travel with its code instead of living only in one operator's local, gitignored file.

### Implemented (build order item 1, commit `5f661d4`)

`Config` (`src/config.rs`) gained `projects: Vec<ProjectPointer>`, `#[serde(default)]` so today's single-project `lucid.toml` shape keeps loading unchanged. `ProjectPointer` is just `{ path: PathBuf }` — the pointer described above, nothing more.

Each pointed-to repo's own file is named `lucid.project.toml` — a filename this page hadn't specified before implementation; it's now the fixed name `ProjectConfig::load` reads from a project's `path`. `ProjectConfig` holds `project_key: Option<String>`, `verify_cmd: Option<String>`, and `base_branch: String` (`#[serde(default = "default_base_branch")]`, defaulting to `"main"`).

Validation is opt-in, not eager: `Config::validate_projects()` resolves and parses every configured project's `lucid.project.toml`, returning a per-project error naming the offending path if one is missing or malformed. It's only called from the `lucid config validate` CLI command (`src/main.rs`) — every other command that calls `Config::load()` doesn't touch `[[projects]]` at all today, so a broken per-project config file stays silent until an operator explicitly runs `config validate`.

**Open, unresolved**: `TrackerConfig.project_key` (existing, global, under `[tracker]`) and `ProjectConfig.project_key` (new, per-repo, in `lucid.project.toml`) now both exist with identical meaning. Nothing consumes or reconciles the per-project one yet — it's inert until the daemon loop (build order items 2-3) actually wires per-project dispatch through the tracker. Which one wins, or whether they need to be merged into one field, is not decided.

## Daemon loop

`Daemon::tick()` already does reconcile → presence-check → dispatch → PM-wake in one pass per call. This wraps that in `for project in &self.projects`, still fully sequential for `Local`-execution dispatches (see [sandboxed-execution](sandboxed-execution.md) for when that constraint relaxes) — one project's dispatch completes before the next project's starts, within a tick, matching the daemon's existing "deliberately sequential" design rather than introducing a second concurrency model to reason about.

`DaemonState.runs` and `last_pm_wake` need to become keyed by project id (`HashMap<ProjectId, ProjectState>`) instead of flat — otherwise two projects' PM-wake backoff timers and retry-tracking collide on the same map.

**Known sharp edge, not yet resolved**: `FileTracker`'s issue-id scheme (`LOCAL-{n}`, a counter local to one JSON file) would collide across two separate `FileTracker`-backed projects if they're ever run against a shared id-namespace — today each `FileTracker` instance has its own file, so this is fine as long as `runs`/`DaemonState` keying stays project-scoped too (see above). Worth a test once multi-project is real, not before.

## CLI: directory detection, not a persistent stateful context

Grounded via `research-first` (2026-08-19), comparing three real patterns:

- **Stateful context switching** (kubectl `set-context`, `docker context use`) — a persistent "current context" written to a config file, `--context` overrides per-call. Checked directly against Docker's own issue tracker while researching this: open issues there are literally titled *"confusing warning when context is overridden"* and *"show when context is overridden by DOCKER_HOST"* — the pattern's own maintainers field regular confusion from users who forgot which context was active.
- **File-based discovery + flag** (`flyctl`) — reads `fly.toml` from the current directory, `-a` overrides.
- **Directory detection** (`gh`) — infers the target from the git remote of the current directory, `--repo` overrides.

Lucid already resolves `lucid.toml` by directory/file discovery (`--config` → `./lucid.toml` → XDG fallback), closer to the `flyctl`/`gh` shape than kubectl/Docker's. Adopting a persistent stateful context would be a new pattern for lucid to learn, and it's the one pattern whose own maintainers visibly field user confusion about it. **Decision: `lucid task create`/`approve`/`list`/`dispatch-now` gain `--project <name>`, defaulting to whichever configured project's `workdir` contains the current directory** — zero flags needed for the common case (you're sitting in the repo you mean), explicit override always available, no silent persistent state to forget.

## Build order

1. `[[projects]]` pointer array in `lucid.toml` + per-repo checked-in config file shape (the `WORKFLOW.md`-equivalent). **Done** — see Implemented section above.
2. `DaemonState` re-keyed by project id.
3. `Daemon::tick()` loops projects sequentially.
4. `--project <name>` CLI flag + directory-detection default across the `task` subcommands.
5. `FileTracker` id-collision check once multiple projects can share a daemon process.

(3) depends on (1) and (2); (4) is independent and can land any time after (1).
