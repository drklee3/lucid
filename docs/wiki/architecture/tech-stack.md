# Tech Stack: Rust

## Decision, superseding an earlier wrong turn

The first pass at this reasoned "presence detection needs D-Bus, Python's D-Bus bindings are more mature, therefore Python." That's a scoping error: presence detection is one narrow subsystem (a background watcher reading a couple of `systemd-logind` properties over D-Bus), not something that should determine the language for the whole system — and the maturity claim didn't hold up on inspection anyway. `zbus` is a mature, actively-maintained, async-native Rust D-Bus crate, arguably more modern than the Python options it was being compared against.

Once the language choice and the D-Bus-library choice are separated and checked honestly, Rust is the better fit on the merits, independent of preference:

- **Long-running system daemon.** A compiled binary with no runtime/interpreter dependency is a better fit for a `systemd --user` service than an interpreted-language process: faster startup, lower idle memory footprint, nothing extra to keep on `PATH`.
- **State machine correctness.** The system is, at its core, several explicit state machines (Worker phases, tracker decision state, presence mode). Rust enums carry data per variant (not C-style tags) and `match` is compiler-enforced exhaustive — adding a new state and forgetting to handle it somewhere is a compile error, not a live bug. This directly targets a failure pattern that showed up repeatedly in the [state-machine gap analysis](state-machine-gaps.md): OpenHands' `ERROR` state marked "optional for future use" in its own source, cyrus's cluster of silent-failure issues from states not being fully surfaced, Symphony's blocked-state map not persisting across restarts. All three are "a state existed but wasn't fully handled" bugs — exactly the class Rust's exhaustiveness checking catches at compile time. This matters more here than in a typical CRUD app, since the design already extends Symphony's state machine with states nothing surveyed had (an "awaiting human input" state, a "stuck/looping" state distinct from stall-timeout).
- **Concurrency without a new runtime paradigm.** The daemon tracks multiple in-flight Worker sessions concurrently — Symphony's own reason for existing. `tokio` async tasks plus Rust's ownership model give the same class of safety Symphony gets from Elixir/BEAM's actor model, without introducing an unfamiliar runtime.

## Crate choices

| Concern | Crate | Why |
|---|---|---|
| CLI | `clap` | Best-in-class across any language |
| Async runtime | `tokio` | Poll loop, concurrent session tracking |
| D-Bus / presence | `zbus` | Async-native, no disadvantage vs. any other language here |
| Linear GraphQL | `reqwest` + `serde` | Typed structs for the specific queries/mutations behind the tracker-adapter interface — a real advantage over Python's dict-shaped JSON for a deterministic backend |
| Local state store | `rusqlite` | Daemon's own bookkeeping (running/blocked/retry state, session identity for continuation-turn resume) — specifically so a restart doesn't lose in-flight state the way Symphony's does |
| Git worktree + harness dispatch | `std::process::Command` / `tokio::process` | Shelling out to `git` and to whichever harness CLI — no library needed, same approach Symphony and cyrus both take regardless of their own implementation language |

## Not a dependency

Hermes's own subsystems (its sandbox machinery, its `cron/suggestions.py` pattern, its MCP wiring) were evaluated and explicitly rejected as dependencies — see [overview: architectural correction](overview.md#architectural-correction-standalone-not-embedded-in-hermes). What's still genuinely reusable is reframed as *optional convenience*, not a dependency: Linear's MCP server is Linear's own product, callable by any MCP client; Hermes-the-harness is invoked the same way `claude -p` or `codex exec` would be, a CLI subprocess call.

## Naming note

`lucid` is already a taken crate name on crates.io. Irrelevant for a private/local project — never need to publish there. If this ever goes public, publish under a name like `lucid-orchestrator` without renaming the repo.

Source: `docs/design.md` § High-Level Components & Tech Stack → Implementation language: Rust.
