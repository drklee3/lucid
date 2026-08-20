# Multi-Project: One Daemon Instance, Many Repos

One running `lucid start` process manages several repos at once, rather than one process per repo: `lucid.toml` carries a `[[projects]]` pointer array, and `Daemon` walks every configured project each tick, each with its own tracker binding and `workdir`. A `lucid.toml` with no `[[projects]]` keeps today's original single-repo shape unchanged.

## What Symphony already does here

[Symphony patterns](symphony-patterns.md) already documents the relevant precedent: `WORKFLOW.md`, a **repo-owned agent contract, checked into and versioned with each repo's own code** — tracker binding, polling interval, workspace/hook config, all declared by the repo itself, not centralized in the orchestrator's own config. Multi-repo support in Symphony isn't "one config listing N repos"; it's "N repos, each self-declaring how it wants to be worked on."

`lucid.toml` today is the opposite of that: `/lucid.toml` is gitignored, treated as local runtime state alongside `.env`, not a versioned contract. That's a real inconsistency with a pattern the wiki's own research already flagged as worth adopting ("WORKFLOW.md pattern — repo-owned agent contract versioned with code; universal praise across systems surveyed" — [symphony-patterns.md](symphony-patterns.md)).

## Split: global daemon config vs. per-project, repo-owned config

**Stays global** (one instance, shared across every project): presence detection, observability/OTel config, tick interval, stall timeout. There's exactly one human whose "away" state matters — it doesn't vary by which repo is being worked on, so presence gating stays a single top-level concern, not something threaded per-project.

**Becomes per-project**: tracker binding (team/project/backend), `workdir`, `base_branch`, `worktree_root`, `verify_cmd`, and dispatch retry-tracking (`runs`).

Config shape: the central `lucid.toml` gains a `[[projects]]` array (same pattern `[[harness_profiles]]` already uses) — each entry a **pointer**, not a full config: `path = "/home/drk/github/repo-a"`. Each pointed-to repo carries its own lightweight checked-in config file for the repo-specific bits (tracker project key, `verify_cmd`, `base_branch`) — Symphony's `WORKFLOW.md` shape, adapted. The daemon's own config says *which repos to watch*; each repo's own file says *how it wants to be worked on*. A project's settings travel with its code instead of living only in one operator's local, gitignored file.

### Implemented (build order item 1, commit `5f661d4`)

`Config` (`src/config.rs`) gained `projects: Vec<ProjectPointer>`, `#[serde(default)]` so today's single-project `lucid.toml` shape keeps loading unchanged. `ProjectPointer` is just `{ path: PathBuf }` — the pointer described above, nothing more.

Each pointed-to repo's own file is named `lucid.project.toml` — a filename this page hadn't specified before implementation; it's now the fixed name `ProjectConfig::load` reads from a project's `path`. `ProjectConfig` holds `project_key: Option<String>`, `verify_cmd: Option<String>`, `base_branch: String` (`#[serde(default = "default_base_branch")]`, defaulting to `"main"`), and `ticket_source: TicketSource` (`#[serde(default)]`, defaulting to `OperatorOnly` — see [sandboxed-execution](sandboxed-execution.md) § Trust routing for what this field gates).

Validation is opt-in, not eager: `Config::validate_projects()` resolves and parses every configured project's `lucid.project.toml`, returning a per-project error naming the offending path if one is missing or malformed. It's only called from the `lucid config validate` CLI command (`src/main.rs`) — every other command that calls `Config::load()` doesn't touch `[[projects]]` at all today, so a broken per-project config file stays silent until an operator explicitly runs `config validate`.

`TrackerConfig.project_key` (existing, global, under `[tracker]`) and `ProjectConfig.project_key` (per-repo, in `lucid.project.toml`) both exist with identical meaning; see the Daemon loop section below for which one wins now that `effective_tracker_config()` reconciles them.

## Daemon loop

`Daemon::tick()` walks every configured project sequentially, one at a time — no second concurrency model layered on top of the daemon's own mixed sequential/concurrent dispatch design (`Sandboxed` issues dispatch concurrently with each other, `Local` issues stay sequential, within one project's tick — see [sandboxed-execution](sandboxed-execution.md)).

`DaemonState.runs` is keyed by project id (`HashMap<ProjectId, ...>`) rather than flat, so two projects' retry-tracking doesn't collide on the same map.

### Implemented (build order item 3, commit `f72c75b`)

`Daemon` (`src/daemon.rs`) no longer holds tracker/workdir/base_branch/verify_cmd/runs directly; those moved onto a new private `ProjectRuntime` struct, one instance per configured project, held as `projects: Vec<ProjectRuntime>` on `Daemon`. `Daemon` itself keeps only what's genuinely shared: harness profiles, observability config, presence sources/config, tick/stall timing, and the override file/audit log. `Daemon::new` now returns `anyhow::Result<Self>` instead of `Self`, since building a project's tracker (or loading its `lucid.project.toml`) can fail — with `[[projects]]` empty, it builds exactly one `ProjectRuntime` wrapping `config.daemon.*` directly (today's single-repo shape, unchanged); otherwise one `ProjectRuntime` per pointer, each loading its own `ProjectConfig` and its own tracker via `effective_tracker_config()` (below).

`tick()` sequences as: for each project in order, `tick_project()` runs `reconcile_needs_review()` then `dispatch_approved_issues()` (dispatch only runs if reconcile succeeded, matching prior single-project sequencing) — then, once every project's reconcile/dispatch has been attempted, presence resolves exactly once, globally, purely to feed the audit log (see [presence detection](presence-detection.md)). A given project's `tick_project()` error is caught, logged (`project `{id}`: tick failed: {e}`), and does not stop the remaining projects' reconcile/dispatch from running — but `tick()` still remembers the first such error and returns it once every project (and presence resolution) has been handled, so `run_foreground`'s caller-level log still fires.

### `project_key`: `ProjectConfig` wins over `TrackerConfig` when set

`effective_tracker_config()` (`src/daemon.rs`) resolves this: a project's own declared `project_key` (from its `lucid.project.toml`) wins when set; the operator's central `[tracker]` block's `project_key` is the fallback when a project doesn't declare its own. Everything else on `TrackerConfig` — `backend`, `team_key`, `api_key_env`, `managed_label` — stays global, applied identically to every project's tracker; only `project_key` is something a repo plausibly wants to declare for itself, since it's the one field that names *which* Linear project a given repo's issues live under.

**Known sharp edge, not yet resolved**: `FileTracker`'s issue-id scheme (`LOCAL-{n}`, a counter local to one JSON file) would collide across two separate `FileTracker`-backed projects if they're ever run against a shared id-namespace. Each `FileTracker` instance has its own file (one per project's `TrackerConfig.file_path`), and `runs`/`DaemonState` keying is project-scoped (see above), so this is still fine in practice now that multi-project ticks are real and running — but it's untested, and build order item 5 below is exactly this check.

## CLI: directory detection, not a persistent stateful context

Grounded via `research-first` (2026-08-19), comparing three real patterns:

- **Stateful context switching** (kubectl `set-context`, `docker context use`) — a persistent "current context" written to a config file, `--context` overrides per-call. Checked directly against Docker's own issue tracker while researching this: open issues there are literally titled *"confusing warning when context is overridden"* and *"show when context is overridden by DOCKER_HOST"* — the pattern's own maintainers field regular confusion from users who forgot which context was active.
- **File-based discovery + flag** (`flyctl`) — reads `fly.toml` from the current directory, `-a` overrides.
- **Directory detection** (`gh`) — infers the target from the git remote of the current directory, `--repo` overrides.

Lucid already resolves `lucid.toml` by directory/file discovery (`--config` → `./lucid.toml` → XDG fallback), closer to the `flyctl`/`gh` shape than kubectl/Docker's. Adopting a persistent stateful context would be a new pattern for lucid to learn, and it's the one pattern whose own maintainers visibly field user confusion about it. **Decision: `lucid task create`/`approve`/`list`/`dispatch-now` gain `--project <name>`, defaulting to whichever configured project's `workdir` contains the current directory** — zero flags needed for the common case (you're sitting in the repo you mean), explicit override always available, no silent persistent state to forget.

### Implemented (build order item 4, commit `813d06a`)

`lucid task create`/`approve`/`reject`/`list`/`dispatch-now` all gained `--project <name>` (`src/cli.rs`). Resolution happens fresh on every invocation via `resolve_project()` (`src/main.rs`) — nothing persistent is written, matching the "not a stateful context" decision above: an explicit `--project` wins if given, otherwise whichever configured `[[projects]]` entry's `path` contains the current working directory. A project has no separate `name` field in `[[projects]]`; the CLI addresses it by the final path component of its pointer `path` (`project_name()`). Zero or more than one directory match, or a `--project` naming an unconfigured project, is a hard error listing the configured project names rather than guessing. Repos with no `[[projects]]` configured are unaffected: `--project` given with none configured errors, omitted is a no-op — identical to behavior before this change.

Only `dispatch-now` changes runtime behavior on a resolved project: it applies the project's `path`/`base_branch`/`verify_cmd` as overrides for `config.daemon.workdir`/`base_branch`/`verify_cmd` before dispatching. `list`/`create`/`approve`/`reject` accept and validate `--project`, but don't yet change tracker scoping — `resolve_project()` is called for its validation/error behavior only. `Daemon::tick()` is what actually applies `ProjectConfig.project_key` (via `effective_tracker_config()`, see the Daemon loop section above) to scope a project's tracker queries; these CLI subcommands don't yet route through that same resolution.

## Build order

1. `[[projects]]` pointer array in `lucid.toml` + per-repo checked-in config file shape (the `WORKFLOW.md`-equivalent). **Done** — see Implemented section above.
2. `DaemonState` re-keyed by project id. **Done** — `runs` is `HashMap<ProjectId, _>` on `DaemonState` (`src/state.rs`, commit `bab7e22`).
3. `Daemon::tick()` loops projects sequentially. **Done** — see Implemented section above.
4. `--project <name>` CLI flag + directory-detection default across the `task` subcommands. **Done** — see Implemented section above.
5. `FileTracker` id-collision check once multiple projects can share a daemon process.

(3) depends on (1) and (2); (4) is independent and can land any time after (1).
