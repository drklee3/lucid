# lucid

Named for lucid dreaming: it acts on its own, but stays directed within bounds you set — never fully unsupervised.

Coding-agent systems stack into layers — model, harness, what a harness does with sub-agents inside one task, what strings tasks together into a project, and who decides a task should exist at all. Each layer swaps independently of the ones around it. lucid owns exactly one of them: it's the part that takes a queue of human-approved tickets and turns them into an ongoing stream of dispatched, reviewed, merged PRs — nothing above that (deciding what's worth building) and nothing below it (how any single task actually gets solved).

| Layer | What it is | Swappable via | lucid's relationship |
|---|---|---|---|
| Task origination | Deciding a ticket should exist | Human, or any external cron/agent, filing through the tracker | Can't tell them apart — doesn't try to |
| **Inter-task orchestration** | **Sequencing many one-shot tasks into an ongoing project** | **lucid itself** | **Lives here** — queue → worktree → sandboxed dispatch → PR, one ticket at a time |
| Intra-task composition | How one task actually gets solved inside a harness — one model, or a harness spawning its own sub-agents | The harness's own internal design | Invisible — one dispatch, one process, one diff back either way |
| Agent harness | Bounded process: starts, does a task, exits | `HarnessProfile.cmd`/`args` (`claude`, `codex`, `hermes`, pi, ...) | Spawns it, sandboxes around it, never reaches inside it |
| LLM API | The model | Whatever the harness calls | Never calls one directly — model-agnostic by construction |

Concretely, that means:

- **Dispatches** approved tickets to a coding harness (`claude -p`, `codex exec`, ...) in an isolated git worktree, sandboxed by default — runs continuously, whether you're at the keyboard or not, since each ticket was already human-approved.
- **Reconciles**: retries stalled runs, opens/merges PRs via `gh`, reports back.
- Tracker-agnostic, harness-agnostic, model-agnostic by design.

See [`docs/wiki/index.md`](docs/wiki/index.md) for full architecture and resolved decisions, and [`docs/FEATURES.md`](docs/FEATURES.md) for what's built vs. still open.

## How it fits together

```mermaid
flowchart TB
    subgraph daemon["lucid daemon (Rust, always-on)"]
        direction TB

        subgraph worker_block["Dispatch pipeline (core loop, presence-independent)"]
            direction LR
            worktree["Per-issue git worktree"] --> exec["Harness dispatch\nsandboxed by default"] --> pr["PR open / merge\nvia gh"]
        end

        presence["Presence watcher\nidle/active detection\n(feeds the audit log)"]

        reconcile["Reconciliation loop\npoll · stall-detect · retry"]

        tracker -->|approved issue| worker_block
        reconcile --> worker_block
        reconcile --> tracker
    end

    external["Human, or any external\ncron/agent proposing work"]
    tracker[("Tracker adapter\nLinear / file-backed")]
    harness[["Coding harness\nclaude / codex / ..."]]
    github[["GitHub (gh)"]]
    otel[("OTel / Phoenix\ntrace correlation")]

    external -->|files a ticket| tracker
    exec <-->|subprocess| harness
    pr <--> github
    exec -.->|traces| otel

    classDef ext fill:#e8e8e8,stroke:#888,color:#333;
    class tracker,harness,github,otel,external ext;
```

One small standalone process, no framework. Two other things worth knowing at a glance:

**Every backend is swappable** — same shape everywhere, a trait with a config-selected implementation:

| Backend | Built-in options |
|---|---|
| Tracker | Linear, local file |
| Presence source | logind (dead on WSL2), manual override |
| Harness execution | Sandboxed (Docker), Local |
| Notifications | none (default), script |

**Any of those can be a plain script instead of Rust**, not just a compiled-in option — drop an executable in `.lucid/notify/on_needs_review` and it's a complete Discord/Slack/anything integration, no code change to lucid itself. Ready-to-copy examples: [`docs/notify-scripts/`](docs/notify-scripts/). Design details: [extensibility primitives](docs/wiki/architecture/extensibility-primitives.md).

Full breakdown: [`docs/wiki/architecture/overview.md`](docs/wiki/architecture/overview.md) for the component categories, [`docs/FEATURES.md`](docs/FEATURES.md) for built-vs-open.

## Quickstart

Requires `git` and an authenticated [`gh`](https://cli.github.com/) on `PATH` — every dispatch pushes a branch and opens/merges its PR through `gh`.

```bash
cargo build --release
export PATH="$PWD/target/release:$PATH"

# write lucid.toml — see Configuration below for what goes in it
docker compose up -d          # optional: Arize Phoenix, for trace correlation
lucid config validate
lucid presence override autonomous   # logind auto-detection isn't wired up yet
lucid start --foreground
```

Full command reference: [`docs/CLI.md`](docs/CLI.md).

## Configuration

`lucid` reads a single TOML file (`./lucid.toml` by default). Minimal working example, file-backed tracker, no credentials needed:

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

[observability]
otlp_endpoint = "http://localhost:4317"
trace_ui_base_url = "http://localhost:6006"
```

Full field reference (every section, every default): [`docs/CONFIGURATION.md`](docs/CONFIGURATION.md).
