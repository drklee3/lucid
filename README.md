# lucid

Autonomous, presence-aware development orchestration. Named for lucid dreaming —
acts on its own, but stays directed within bounds you set; never fully unsupervised.

A standalone Rust daemon that, while you're away (presence-gated, not on a naive
schedule), investigates a repo against a stated goal, flags concrete gaps as
proposals for you to approve, and dispatches approved work to a coding harness
(`claude -p`, `codex exec`, or others) — reconciling state, retrying, and
reporting back the same way it would if you were watching.

Tracker-agnostic, harness-agnostic, model-agnostic by design. See
[`docs/wiki/index.md`](docs/wiki/index.md) for the full architecture, resolved
decisions, and grounding research; [`docs/FEATURES.md`](docs/FEATURES.md) for
exactly what's built vs. still open.

## Status

Working MVP, live-tested against real `claude -p` subscription dispatch — not a
prototype, but not feature-complete either:

- ✅ Presence-gated reconciliation loop (`lucid start`), PM gap-detection wake,
  Claude Code dispatch with block/timeout handling, file-backed and real Linear
  tracker adapters, OTel trace correlation back to the tracker item, per-issue git
  worktree isolation with PR-based completion (every dispatch gets its own
  branch/worktree; `lucid` pushes and opens a PR via `gh`, merging it itself when
  `ReviewMode` says the task can close without a human).
- ⛔ No cross-process `status`/`stop` (needs an IPC layer, not designed yet),
  state is in-memory only (no restart persistence).

See [`docs/FEATURES.md`](docs/FEATURES.md) for the itemized breakdown.

## Quickstart

Requires `git` and an authenticated [`gh`](https://cli.github.com/) on `PATH` —
every dispatch pushes a branch and opens/merges its PR through `gh`.

```bash
cargo build --release

# write lucid.toml — see Configuration below for what goes in it
docker compose up -d          # optional: Arize Phoenix, for trace correlation
./target/release/lucid config validate
./target/release/lucid presence override autonomous   # logind auto-detection isn't wired up yet
./target/release/lucid start --foreground
```

Full command reference: [`docs/CLI.md`](docs/CLI.md).

## Configuration

`lucid` reads a single TOML file, resolved in this order: `--config <path>`,
then `./lucid.toml`, then `$XDG_CONFIG_HOME/lucid/config.toml`. Validate it
with `lucid config validate` before starting the daemon.

Minimal working example (file-backed tracker, no Linear credentials needed):

```toml
[[harness_profiles]]
name = "claude-subscription"
kind = "ClaudeCode"
cmd = "claude"
args = ["-p"]
auth_mode = "Subscription"
priority = 1

[tracker]
backend = "file"
file_path = "state/tracker.json"

[presence]
idle_threshold_minutes = 20
proposal_cap_per_wake = 3

[observability]
otlp_endpoint = "http://localhost:4317"
trace_ui_base_url = "http://localhost:6006"
```

### `[[harness_profiles]]`

One entry per dispatch target; multiple profiles for the same harness (e.g. a
subscription profile plus an API-key fallback) run in `priority` order.

| Field | Type | Notes |
|---|---|---|
| `name` | string | Free-form label, shown in dispatch logs/status. |
| `kind` | `"ClaudeCode"` \| `"Codex"` | Which harness binary this profile drives. |
| `cmd` | string | The binary to invoke (e.g. `claude`, `codex`). |
| `args` | string[] | Static args; the task prompt is appended at dispatch time. |
| `auth_mode` | `"Subscription"` \| `"ApiKey"` | `Subscription` reads the harness's existing login; `ApiKey` forces metered billing (e.g. `ANTHROPIC_API_KEY`/`OPENAI_API_KEY`). |
| `priority` | integer | Lower runs first. |

### `[tracker]`

| Field | Type | Notes |
|---|---|---|
| `backend` | `"file"` \| `"linear"` | `file` needs no credentials; `linear` talks to Linear's real GraphQL API. |
| `file_path` | string | Required for `backend = "file"`. Where the JSON store lives. |
| `api_key_env` | string | Required for `backend = "linear"`. Env var holding the API key (e.g. `LINEAR_API_KEY`). |
| `team_key` | string | Required for `backend = "linear"`. Linear's short team key (e.g. `ENG`), not its UUID. |
| `project_key` | string | Optional, `linear` only. Scopes lucid to one Linear project within `team_key` — Linear issues don't require a project, so omitting this operates team-wide. |

### `[presence]`

| Field | Type | Notes |
|---|---|---|
| `idle_threshold_minutes` | integer | Minutes of sustained idle before flipping to autonomous mode. |
| `proposal_cap_per_wake` | integer | Max proposals a single PM wake cycle may file. |
| `override_path` | string | Optional. Defaults to `$XDG_STATE_HOME/lucid/presence-override` (or `~/.local/state/...`). |

### `[observability]`

| Field | Type | Notes |
|---|---|---|
| `otlp_endpoint` | string | Where dispatched harnesses send OTel traces/logs (e.g. Phoenix's `http://localhost:4317`). |
| `trace_ui_base_url` | string | Base URL of the trace UI, used to build the trace link posted back to the tracker item. |
| `trace_ui_project_id` | string | Optional. Falls back to `"default"` (Phoenix's default project). |
| `log_prompts` | bool | Optional, defaults to `false`. Opt-in prompt/tool-content capture — this is the point where the trace store starts holding sensitive content. |

### `[daemon]` (optional — every field has a default)

| Field | Type | Default | Notes |
|---|---|---|---|
| `tick_interval_secs` | integer | `30` | How often the reconciliation loop checks presence and dispatches approved issues. |
| `stall_timeout_secs` | integer | `600` | Hard wall-clock limit before a harness process is killed and marked `TimedOut`. |
| `pm_wake_interval_mins` | integer | `60` | Minimum time between PM gap-detection wake cycles while autonomous. |
| `workdir` | string | `"."` | The main repo checkout. Every dispatch's worktree branches off `base_branch`'s tip here, and this is where `gh pr create`/`gh pr merge` run from. |
| `base_branch` | string | `"main"` | Branch each dispatch's worktree is created from, and PRs target. |
| `worktree_root` | string | system temp dir | Where per-issue worktrees are created — kept outside `workdir` so they never show up in the main repo's own `git status`. |
| `verify_cmd` | string | unset | Repo-wide default command for `ReviewMode::Agent`'s verify step (e.g. `cargo test`); a per-task `verify_cmd` overrides it. |
