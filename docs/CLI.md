# CLI

v1 observability is CLI-only (see `docs/wiki/architecture/observability.md`) — the web
dashboard is explicitly deferred. Two commands were resolved as the starting point
(`start`, `status`); this is the full command tree built out from that, kept to what
the current architecture actually supports.

```
lucid start                          Start the orchestrator daemon
lucid stop                           Stop a running daemon

lucid status                         List running/blocked/retrying agents
lucid show <worker-id>               Inspect one worker's session in detail

lucid pm wake                        Manually trigger a PM gap-detection wake cycle

lucid presence status                Show current presence mode and source readings
lucid presence override <mode>       Force presence mode (active | autonomous | auto)

lucid config validate                Validate the config file without starting anything
lucid config show                    Print resolved config (secrets redacted)

lucid task create <title>            File a new proposal directly
lucid task list                      List tracker issues in a given decision state
lucid task approve <issue-id>        Approve an issue for dispatch
lucid task reject <issue-id>         Reject an issue
lucid task dispatch-now <issue-id>   Dispatch one already-approved issue immediately
```

## `lucid start`

Starts the orchestrator: presence watcher, reconciliation tick, PM wake scheduling.

```
lucid start [--foreground] [--config <path>]
```

| Flag | Default | Meaning |
|---|---|---|
| `--foreground` | off | Run attached to the terminal instead of detaching. Detached-by-default in v1 is a stub — no systemd unit is written yet, so `--foreground` is effectively required until that lands. |
| `--config <path>` | `./lucid.toml` or `$XDG_CONFIG_HOME/lucid/config.toml` | Config file path (harness profiles, tracker settings, thresholds). |

## `lucid stop`

Sends a graceful shutdown request to a running daemon (via its control socket/PID
file — mechanism TBD at implementation time, not designed yet beyond "it exists").

```
lucid stop
```

## `lucid status`

The actual v1 observability surface — Symphony's Blocked/Retrying/Running table
content, as CLI output instead of a web dashboard.

```
lucid status [--format table|json] [--watch]
```

| Flag | Default | Meaning |
|---|---|---|
| `--format` | `table` | `table` for human reading, `json` for scripting. |
| `--watch` | off | Live-refresh instead of a single snapshot. |

Example `table` output (columns per Symphony's dashboard content model):

```
ISSUE    STATE      SESSION   RUNTIME  LAST EVENT              RETRIES
ENG-142  Running    a1b2c3    4m12s    StreamingTurn            0
ENG-139  Blocked    9f8e7d    -        awaiting-input (2h ago)  0
ENG-133  Retrying   -         -        rate_limit (30s ago)     2
```

`STATE` values come directly from the Worker phase enum (`src/state.rs`) — this
table is a rendering of that state machine, not a separate concept.

## `lucid show <worker-id>`

Full detail for one worker: phase history, harness used, worktree path, last N log
lines, tracker issue it's tied to.

```
lucid show <worker-id> [--format table|json] [--log-lines <n>]
```

## `lucid pm wake`

Manually trigger a PM gap-detection cycle — for testing without waiting on the
presence gate and wake interval.

```
lucid pm wake [--respect-presence] [--dry-run]
```

| Flag | Default | Meaning |
|---|---|---|
| `--respect-presence` | off | By default, manual wake bypasses the presence gate (it's a deliberate manual trigger). Pass this to require the normal autonomous-mode gate anyway. |
| `--dry-run` | off | Run the gap-detection pass and print what *would* be filed, without calling `create_proposal`. |

## `lucid presence status`

Shows the current resolved mode plus each configured source's individual reading —
useful for debugging exactly why the system is or isn't autonomous (this matters
given the WSL2 `logind` gap already logged in `docs/wiki/architecture/presence-detection.md`).

```
lucid presence status [--format table|json]
```

Example:

```
MODE: active (override: none)

SOURCE            READING       IDLE SINCE
override          -             -
logind             idle (stale)  9d 4h  (WARN: no seat on this host — see docs/wiki)
last-activity-log  active         -
```

## `lucid presence override <mode>`

Sets or clears the explicit override layer (top-priority signal — see `docs/wiki/architecture/presence-detection.md`).

```
lucid presence override active      # force active (never autonomous) until cleared
lucid presence override autonomous  # force autonomous now, regardless of sources
lucid presence override auto        # clear override, return to automatic source-based detection
```

## `lucid config validate` / `lucid config show`

```
lucid config validate [--config <path>]
lucid config show [--config <path>] [--format toml|json]
```

`config show` redacts anything that looks like a credential (API keys, tokens) —
never prints secrets, even local-only ones.

## `lucid task`

A terminal convenience over the tracker's own UI (Linear), not a second source of
truth — every subcommand goes through the same `TrackerAdapter` the daemon itself
uses (`set_decision_state`/`query_by_label`), so e.g. `lucid task approve` has the
identical effect to approving the issue directly in Linear. See
`docs/wiki/architecture/worker-completion.md`.

```
lucid task create <title> [--summary <text>] [--why-now <text>]... [--effort small|medium|large]
                   [--risk-note <text>] [--task-type <text>] [--target-path <path>]...
                   [--acceptance-criteria <text>]... [--review auto|human|agent]
                   [--verify-cmd <cmd>] [--config <path>]
lucid task list [--state pending|approved|rejected|done|needs-review] [--format table|json] [--config <path>]
lucid task approve <issue-id> [--config <path>]
lucid task reject <issue-id> [--config <path>]
lucid task dispatch-now <issue-id> [--config <path>]
```

| Flag | Default | Meaning |
|---|---|---|
| `--state` | `approved` | Which decision state to list. Only the states with a CLI-reachable meaning today — `StaleClosed` (auto-stale-close, not built yet — see `docs/FEATURES.md` § Tracker adapter) isn't exposed here. |
| `--format` | `table` | `table` for human reading, `json` for scripting. |

`lucid task create` files a new `Proposal` directly through the tracker adapter —
the same `create_proposal` write path `pm::wake` uses, without its
`query_similar` dedup check (a human typing a title explicitly isn't the
runaway re-filing case that check guards against). Every flag but `<title>` is
optional: `--summary` defaults to the title, `--effort` defaults to `medium`,
`--task-type` defaults to `task`, `--review` defaults to `auto`. `--why-now`,
`--target-path`, and `--acceptance-criteria` are each repeatable. On success it
prints the new issue id (e.g. `LOCAL-1` for the file backend) — pass that id to
`lucid task approve`. This is the only CLI path that sets `Proposal.review` or
`Proposal.verify_cmd` (see `docs/wiki/architecture/worker-completion.md`); once
filed, neither is updatable except by hand-editing the ticket.

On the Linear backend, `--review agent` requires the `review:agent` label to
already exist in the workspace — `LinearAdapter::label_id` never creates
labels, so `task create` fails with Linear's own "label not found" error if it
doesn't. Create the label in Linear first.

`lucid task approve`/`reject` only change decision state — there's no `--review`
flag to change `ReviewMode` after creation; that's set once at proposal-filing time
(`Proposal.review`, now settable via `lucid task create --review`) and isn't
currently updatable from the CLI afterward.

`lucid task dispatch-now <issue-id>` runs the *exact* dispatch path the daemon's
regular tick would run for that issue (`worker::dispatch_and_finalize`, shared by
both callers) — it changes **when** approved work runs (now, instead of waiting for
the next tick + presence gate), never **whether** it's allowed to. It requires the
issue already be in the `Approved` state in the tracker and errors out otherwise,
pointing at `lucid task approve` — it is not an independent trigger mechanism.

---

## Not yet designed

Flagged here rather than silently decided: `lucid stop`'s actual IPC mechanism
(control socket vs PID+signal), whether `start` should self-daemonize or require an
external supervisor (systemd unit) in v1, and any command surface for the
review/rework trigger policy (`docs/wiki/architecture/review-rework-ux.md` leaves that decision itself open).
