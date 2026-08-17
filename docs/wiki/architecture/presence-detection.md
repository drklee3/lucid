# Presence Detection

## It's a pluggable source list, not a single mechanism

Same shape as the [tracker adapter](tracker-adapter.md) and the [harness dispatch profile list](harness-dispatch.md): the orchestrator depends on a small internal interface (`async fn is_idle() -> bool`, `async fn idle_since() -> Option<Duration>`), and any number of concrete sources can implement it. `logind` D-Bus is the first/reference implementation, not the only one that will ever exist. Sources compose — any source reporting "not idle" wins (conservative default) — so adding or swapping a source later is additive, not a rewrite.

`PresenceSource` is `async`, not sync-with-a-background-cache: `logind`'s real read is a D-Bus round trip (zbus 5, async-first), and `PresenceSourceList` holds `Box<dyn PresenceSource>`, so it needs `async-trait` (native async-fn-in-trait isn't dyn-compatible) — same reason `TrackerAdapter` uses it. A sync signature backed by a background-task cache was considered and rejected for the reference implementation: it adds a subscription/liveness-tracking layer for no benefit here, since nothing in the orchestrator's reconciliation loop needs `is_idle()` to be non-blocking on a hot path. Revisit only if a future source's read is expensive enough that per-tick blocking becomes a real problem.

Three *kinds* of signal, most authoritative first:

1. **Explicit override** — a state file (owned by the orchestrator, e.g. its own `state/mode`) toggled by an explicit command. Always wins, no debounce.
2. **One or more automatic idle sources** — `logind` D-Bus (`Lock`/`Unlock` signals + `IdleHint`/`IdleSinceHint`) is the reference implementation. Other sources (a Windows-host `GetLastInputInfo` read for WSL2, a macOS IOKit reader, etc.) plug into the same interface later, only when actually needed for whatever environment lucid runs on next.
3. **Last-activity timestamp from any known local agent session log** (Hermes, Claude Code, etc.) — if the user was actively driving any session in the last N minutes, don't flip to autonomous even if the screen looks idle (e.g. they're reading a long tool output). This is a *read*, not a dependency — the orchestrator doesn't need to be hosted by whichever tool it's reading from.

Debounce the transition itself (require idle sustained for the full threshold, not just crossed it) to avoid flapping when the user steps away briefly. Log every mode transition — this is a trust-critical piece of the system and needs an audit trail.

## Why `xprintidle` is out

X11-only, breaks under Wayland. `systemd-logind` over D-Bus is compositor-agnostic (the mechanism `dbus-idle` wraps).

## Verified finding: dead on WSL2

Checked directly on the development machine (WSL2), not assumed:

- `systemd-logind` **is** running and reachable on the system D-Bus (`org.freedesktop.login1`, PID confirmed via `busctl`) — so the `logind` reference implementation is buildable here.
- But its idle tracking is **dead** in this environment. `loginctl list-sessions` showed the active session's `IdleHint=yes` stuck for over a week, because WSL2 has no real "seat" with physical input devices behind it (`CanGraphical=no`, `seat0` has no sessions attached). Nothing inside the WSL2 VM generates HID activity events into logind, so `IdleHint` never resets regardless of actual typing.

This matches the general "idle-detection fragments across platforms" finding in [presence-automation prior art](../research/presence-automation-prior-art.md) — confirmed concretely here rather than just theoretically.

**Consequence, not a blocker:** this doesn't block building the `logind` source (it's still correct, and will work as-is on a real Linux desktop). It means presence-gated autonomy won't actually trigger correctly *on this machine* until a second source (the last-activity-log signal, or eventually a Windows-side input reader) is added to the list. A known gap to close before flipping the system into autonomous mode here — not before starting to build it.

Source: `docs/design.md` resolved decision #1; `docs/research.md` § Presence-Aware Automation Prior Art; query 2026-08-16 (async trait shape, `research-first` dependency audit).
