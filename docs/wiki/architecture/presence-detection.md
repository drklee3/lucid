# Presence Detection

## It's a pluggable source list, not a single mechanism

Same shape as the [tracker adapter](tracker-adapter.md) and the [harness dispatch profile list](harness-dispatch.md): the orchestrator depends on a small internal interface (`async fn is_idle() -> bool`, `async fn idle_since() -> Option<Duration>`), and any number of concrete sources can implement it. `logind` D-Bus is the first/reference implementation, not the only one that will ever exist. Sources compose — any source reporting "not idle" wins (conservative default) — so adding or swapping a source later is additive, not a rewrite.

`PresenceSource` is `async`, not sync-with-a-background-cache: `logind`'s real read is a D-Bus round trip (zbus 5, async-first), and `PresenceSourceList` holds `Box<dyn PresenceSource>`, so it needs `async-trait` (native async-fn-in-trait isn't dyn-compatible) — same reason `TrackerAdapter` uses it. A sync signature backed by a background-task cache was considered and rejected for the reference implementation: it adds a subscription/liveness-tracking layer for no benefit here, since nothing in the orchestrator's reconciliation loop needs `is_idle()` to be non-blocking on a hot path. Revisit only if a future source's read is expensive enough that per-tick blocking becomes a real problem.

Three *kinds* of signal, most authoritative first:

1. **Explicit override** — a state file (owned by the orchestrator, e.g. its own `state/mode`) toggled by an explicit command. Always wins, no debounce.
2. **One or more automatic idle sources** — `logind` D-Bus (`Lock`/`Unlock` signals + `IdleHint`/`IdleSinceHint`) is the reference implementation. Other sources (a Windows-host `GetLastInputInfo` read for WSL2, a macOS IOKit reader, etc.) plug into the same interface later, only when actually needed for whatever environment lucid runs on next.
3. **Last-activity timestamp from any known local agent session log** (Hermes, Claude Code, etc.) — if the user was actively driving any session in the last N minutes, don't flip to autonomous even if the screen looks idle (e.g. they're reading a long tool output). This is a *read*, not a dependency — the orchestrator doesn't need to be hosted by whichever tool it's reading from.

Debounce the transition itself (require idle sustained for the full threshold, not just crossed it) to avoid flapping when the user steps away briefly. Log every mode transition — this is a trust-critical piece of the system and needs an audit trail.

## Audit log: history, not state

The override file (`config::default_override_path`, e.g. `$XDG_STATE_HOME/lucid/presence-override`) holds current decision state — what the orchestrator would read *right now* to know the active override, if any. It gets overwritten in place; it has no memory of what it used to say.

The audit log is the complementary append-only history. `AuditLog` (`src/presence/audit_log.rs`) lives at a fixed sibling path, `presence-audit.log` in the same directory as the override file (`AuditLog::default_path_from_override`). Both files, plus the daemon's own reconciliation-state file, follow the same flat-file-over-database convention — see [persistence](persistence.md) for the full picture across all of them, including why each one handles a corrupt file differently. Every call to `daemon::tick()` resolves a `PresenceMode` (`Active` or `Autonomous`) and compares it against the mode resolved on the previous tick. A line is appended only when the two differ:

```json
{"timestamp":"2026-08-18T12:34:56Z","from":"active","to":"autonomous"}
```

No line is written on the very first tick (nothing to compare against yet) or when the resolved mode is unchanged from the previous tick — the file only ever grows on an actual `Active <-> Autonomous` flip, regardless of which of the three signal kinds above caused it (explicit override, idle source, or activity-log read). The write happens unconditionally once a transition is detected, before any of `tick()`'s Autonomous-only dispatch logic runs, so a transition into or out of Autonomous is always recorded even on a tick that goes on to do nothing else.

This is the mechanism that satisfies the "log every mode transition" requirement above — presence-gated autonomy is trust-critical, and the audit log is what makes a transition inspectable after the fact instead of only visible at the instant it happens.

## What presence gates today: nothing, by design

Presence used to gate `daemon::maybe_wake_pm` — the PM proactively investigating the codebase and filing *new* proposals with no human having looked at them yet. That component doesn't live in lucid anymore (see [overview](overview.md)): proposing work is entirely an external concern, outside lucid's process, so there's nothing left in lucid's own pipeline for presence to gate. `dispatch_approved_issues` (an `Approved` issue already has an explicit human decision behind it) and `reconcile_needs_review` (closing the loop on a PR a human already merged/closed on GitHub) both always ran on every tick regardless of presence mode — see `daemon::tick`'s doc comment.

`daemon::tick` still resolves presence once per tick and feeds it to the audit log (below) — that's an intentionally-kept observability signal, not a live gate. This module (`PresenceSource`, the override file, `lucid presence status`/`override`) stays in lucid because it's a general-purpose pluggable backend, not because anything downstream currently branches on its result. If a future in-daemon feature needs an unsupervised-action gate again, this is where it would plug back in; nothing today requires it to.

## Why `xprintidle` is out

X11-only, breaks under Wayland. `systemd-logind` over D-Bus is compositor-agnostic (the mechanism `dbus-idle` wraps).

## Verified finding: dead on WSL2

Checked directly on the development machine (WSL2), not assumed:

- `systemd-logind` **is** running and reachable on the system D-Bus (`org.freedesktop.login1`, PID confirmed via `busctl`) — so the `logind` reference implementation is buildable here.
- But its idle tracking is **dead** in this environment. `loginctl list-sessions` showed the active session's `IdleHint=yes` stuck for over a week, because WSL2 has no real "seat" with physical input devices behind it (`CanGraphical=no`, `seat0` has no sessions attached). Nothing inside the WSL2 VM generates HID activity events into logind, so `IdleHint` never resets regardless of actual typing.

This matches the general "idle-detection fragments across platforms" finding in [presence-automation prior art](../research/presence-automation-prior-art.md) — confirmed concretely here rather than just theoretically.

**Consequence, not a blocker:** this doesn't block building the `logind` source (it's still correct, and will work as-is on a real Linux desktop). It means presence-gated autonomy won't actually trigger correctly *on this machine* until a second source (the last-activity-log signal, or eventually a Windows-side input reader) is added to the list. A known gap to close before flipping the system into autonomous mode here — not before starting to build it.

Source: initial design/research pass (2026-08-16); query 2026-08-16 (async trait shape, `research-first` dependency audit); query 2026-08-18 (mode-transition audit log).
