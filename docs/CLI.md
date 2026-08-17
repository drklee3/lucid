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

---

## Not yet designed

Flagged here rather than silently decided: `lucid stop`'s actual IPC mechanism
(control socket vs PID+signal), whether `start` should self-daemonize or require an
external supervisor (systemd unit) in v1, and any command surface for the
review/rework trigger policy (`docs/wiki/architecture/review-rework-ux.md` leaves that decision itself open).
