# Persistence: flat files, not a database

## The convention

Every piece of state lucid persists across a daemon restart uses a flat file, not a database. Four instances of the same pattern:

| State | File | Format | Owner |
| --- | --- | --- | --- |
| Presence override | `presence-override` | plain string (`active`/`autonomous`/`auto`) | `presence::override_file::OverrideFile` |
| Mode-transition history | `presence-audit.log` | JSONL, append-only | `presence::audit_log::AuditLog` |
| Local tracker store | (caller-supplied path) | JSON array of issues | `tracker::file::FileTracker` |
| Daemon reconciliation state | `daemon-state.json` | pretty JSON | `state::DaemonState` |

`DaemonState::default_path()` resolves via the same `$XDG_STATE_HOME`-then-`~/.local/state`-then-relative-fallback logic as `config::default_override_path`, just a different filename (`daemon-state.json` instead of `presence-override`) in the same directory. `FileTracker`'s path is caller-supplied rather than fixed, since today it's pointed at a local dev store rather than a real per-install state directory — but the read-whole-file/write-whole-file shape is identical.

This was a deliberate choice, not an oversight: `rusqlite` was in `Cargo.toml` from the initial scaffolding but never wired to anything, and was removed (see `docs/wiki/log.md`) rather than used, once `DaemonState` needed to persist. A single-process daemon reading/writing a handful of small structs once per tick has no need for transactions, indexing, or concurrent-writer support — the things a database buys you. `serde_json::to_string_pretty` plus `std::fs::write` is the whole write path.

## Corruption/missing-file tolerance varies by stakes

All four flat files treat a *missing* file as "start from an empty/default state" — none of them error on first run. They diverge on a *corrupt/unparseable* file:

- `OverrideFile::read` (presence override) treats corrupt content as a hard error (`anyhow::anyhow!("unrecognized override mode...")`). An override file only ever exists because a human or the daemon itself wrote a known-good value moments ago; if it's unparseable, something is actively wrong and surfacing that beats silently guessing.
- `FileTracker::open` also errors on unparseable JSON via `serde_json::from_str`'s `?`.
- `DaemonState::load` is the outlier: both a missing *and* a corrupt/unparseable file fall back silently to `DaemonState::default()`. This is intentional and specific to this file — `DaemonState` is a cache of in-progress reconciliation work (which runs are dispatched, the last resolved presence mode), not a source of truth for anything a human configured by hand. A daemon that refuses to start because its own last-tick snapshot got truncated mid-write is strictly worse than one that starts from empty and re-discovers live state from the tracker on the next tick. Losing this file costs re-dispatch/re-detection work, not correctness.

## Write path

`DaemonState::save(path)` creates the parent directory (`std::fs::create_dir_all`) and writes pretty JSON, called from `Daemon`'s private `save_state()` after every tick in `run_foreground`'s loop — success or failure of the tick itself doesn't matter, the state gets snapshotted either way. A save failure is logged, not propagated: the daemon keeps running on an in-memory state that simply won't survive the next restart, rather than crashing over a disk write.

## When this convention would need to change

If lucid ever needs concurrent readers/writers (a second process, a web dashboard reading live daemon state without going through the daemon), or state large/relational enough that whole-file read/write becomes a real cost, that's the signal to revisit — not before. See [tech-stack](tech-stack.md) for the broader "why Rust, crate choices" reasoning this fits under.
