# Extensibility Primitives

How lucid should offer pi-style "extreme customization" ([pi-harness-extensibility](../research/pi-harness-extensibility.md)) without pi's mechanism, which depends on a JS runtime lucid doesn't have. Resolved here: extensibility is not a separate subsystem bolted onto the daemon — it's a property every trait boundary can optionally have, using one mechanism reused everywhere, not a bespoke hook system plus a bespoke plugin system. The mechanism itself (script contract, invocation modes, wire envelope, schema/versioning, directionality) is decided below and grounded via `research-first`. **Pilot shipped**: `NotificationSink`/`ScriptSink` (one-shot invocation) is built — see § Pilot. Everything else on this page (persistent-mode JSON-RPC, `DispatchPolicy`, `ScriptTracker`) remains design-only; what's left is captured under Open questions.

## The governing rule

**Before adding a new capability as embedded Rust, ask whether it can instead be a generic script-backed implementation of an existing or new trait.** Write bespoke Rust only when one of these holds:

- **Hot path** — called in a tight polling loop (e.g. `reconcile_needs_review` runs every tick) where per-call subprocess spawn cost would matter. A typed built-in stays the fast path; a script-backed implementation is still allowed alongside it for whoever doesn't care about that cost.
- **Security/trust boundary** — e.g. `Config::validate_trust_routing`'s sandboxing requirement for externally-triggered projects (see [sandboxed-execution](sandboxed-execution.md) § Trust routing). A rail that exists specifically so dispatch can't be misconfigured into something unsafe shouldn't itself be overridable by a script — that would just move the trust boundary somewhere unaudited.
- **Correctness-critical typed parsing** — e.g. Linear's real GraphQL response shape, where a typed client catches a schema drift a script parsing JSON by hand wouldn't.

Everything else defaults to: give the trait a generic script-backed implementation, and if no trait covers the point yet, that's a signal to add a narrow one rather than reach for a special-cased hook.

## Why one mechanism, not two

Two mechanisms were considered separately across the conversation this page consolidates: "hooks" (fire scripts at named lifecycle points) and "pluggable backends" (trait + config-selected implementation, lucid's existing `TrackerAdapter`/`PresenceSource`/`HarnessProfile` pattern). They turned out to be the same shape wearing different names — a hook is just a trait method with no built-in Rust implementation. Two pieces of evidence this pattern already works in production, not just theory:

- **`HarnessProfile.cmd`/`args`** — dispatch has been subprocess-based since v1. Any binary speaking the expected contract is a valid harness; nothing about it is Claude/Codex-specific in the daemon's own code.
- **`verify_cmd`** (`ReviewMode::Agent`'s pre-merge gate, see [worker-completion](worker-completion.md)) — already exactly a policy hook: an arbitrary shell command, exit code decides pass/fail, live-tested.

## Primitive categories

| Trait | Typed built-in(s) | Script-backed implementation | Notes |
|---|---|---|---|
| `TrackerAdapter` | Linear, file (`src/tracker/mod.rs`) | `ScriptTracker` — execs `.lucid/tracker/<method>` per call, issue/comment data as JSON on stdin/stdout | Multi-method trait (query, approve, attach_note, attach_link, list_comments); a script adapter needs one script per method or one script dispatching on `$1`/argv |
| `PresenceSource` | logind (`src/presence/mod.rs`) — dead on WSL2, see [presence-detection](presence-detection.md) | `ScriptPresence` — a script returning idle/active (+ optional idle-since timestamp) on stdout | Natural fit for calendar-busy status, a custom sensor, anything logind can't see |
| `NotificationSink` (designed, not yet built — [human-in-the-loop](human-in-the-loop.md)) | `NullSink` | `ScriptSink` from day one | Don't build a bespoke `WebhookSink` — a webhook is a five-line curl script. The generic script adapter *is* the webhook implementation, and also covers Slack/email/anything else without lucid knowing about any of them specifically |
| `ExecutionBackend`/`HarnessProfile` | Sandboxed (Docker), Local | Already fully script-shaped (`cmd`/`args`) | The existing proof-of-pattern; no change needed |
| `DispatchPolicy` (new, not designed beyond this page) | — (no built-in; opt-in only) | A script gate run before dispatch, same shape as `verify_cmd` but pre- rather than post-execution | Covers cases like "block anything touching `payments/`" without a bespoke config DSL |

## The script contract

Discovery: a project-local directory (`.lucid/<kind>/`, mirroring pi's `.pi/extensions/` convention) holding one executable per method/event name. Any language — the daemon only cares about the process boundary.

Two response shapes, matching the two things a script can be asked to do:

- **Gate** (`verify_cmd`, a future `DispatchPolicy`) — exit code decides allow/block; stdout, if present, becomes the reason attached to the tracker note.
- **Query/notify** (`TrackerAdapter` methods, `NotificationSink` events, `PresenceSource`) — structured JSON in and out; a notify call's response is ignored (fire-and-forget), a query call's stdout is deserialized into whatever the trait method returns.

**Invocation mode, resolved:** split by call frequency rather than one mode for everything.

- **One-shot** (`NotificationSink`, `DispatchPolicy`, occasional `PresenceSource` polls) — spawn per call, write JSON to stdin, close stdin (EOF), read one JSON value from stdout until the process exits. No framing needed; the whole stream is the message.
- **Persistent** (`TrackerAdapter` — hit every reconcile tick, spawn-per-call cost would actually matter here) — spawn once, process stays alive for the daemon's lifetime (recycled on failure/exit), newline-delimited **real JSON-RPC 2.0** over stdio: `{"jsonrpc": "2.0", "id": ..., "method": ..., "params": {...}}` per line in, `{"jsonrpc": "2.0", "id": ..., "result": {...}}`/`{"jsonrpc": "2.0", "id": ..., "error": {...}}` per line out. Framing chosen over Content-Length-prefixing (LSP's approach) — a raw newline byte can't appear in compact (non-pretty-printed) JSON output, since a string's newline character is always the two-character escape `\n`, so line-delimiting is safe as long as scripts emit compact JSON, and the failure mode if a plugin violates that (pretty-printed output, hand-rolled bad JSON) is an immediate, loud parse error rather than silent corruption.

  Envelope chosen as the *actual* JSON-RPC 2.0 spec, not a bespoke look-alike — restricted to the subset lucid needs: always-id'd, non-batched requests only (batching and id-less "notification" requests are optional spec features, simply unused here, not non-compliance). The spec's reserved error-code range (-32700 parse error, -32601 method not found, etc.) already covers "this plugin doesn't implement this trait method" for free. This is separate from, and doesn't imply, adopting MCP — MCP's capability-negotiation/tool-discovery machinery is an *application layer* MCP built on top of JSON-RPC, not part of JSON-RPC itself; it's unneeded here because lucid always calls a fixed, known set of trait methods, unlike an LLM client discovering tools at runtime.

  **Grounded via `research-first` (2026-08-19).** Checked against a source that separates JSON-RPC's actual spec ("three message shapes and one error object," transport-agnostic — [imti.co](https://imti.co/mcp-json-rpc/)) from MCP's additions: every commonly-cited JSON-RPC "weakness" found (capability negotiation breaking least privilege, poor discoverability, OAuth-for-stdio gaps, DoS via oversized batches) turned out to be a critique of MCP's application layer or its network-facing transports, not the envelope itself — none apply to a local, fixed-method-set, subprocess-only usage. Corroborating data point: MCP's own 2025-06-18 revision removed batching from its JSON-RPC usage, independently landing where this design already had. **varlink** (systemd's modern JSON-native IPC protocol, actively maintained across C/Go/Python/Rust as of early-to-mid 2026) was surfaced as a legitimate alternative and considered — rejected in favor of JSON-RPC specifically because lucid's plugin authors are expected in arbitrary languages (a TypeScript Forgejo backend, say), and JSON-RPC's ecosystem reach is far broader than varlink's, which skews systemd/C-adjacent. **One correction to the "reuse existing libraries" claim above**: `jsonrpsee` (the actively-maintained Rust JSON-RPC crate, tracking a 1.0 milestone) supports only HTTP/WebSocket transports, not stdio — so lucid's own daemon side hand-rolls the small stdio dispatcher regardless of adopting JSON-RPC's message shape. Not a real cost (the spec is a handful of serde structs), but the "free crate" framing overstated what's actually free; the library-reuse benefit is real for plugin authors in other ecosystems, not for lucid's own implementation.

Not resolved here: timeout defaults per call type, and whether a single script dispatching on an argv verb or one-script-per-method reads better in practice for the one-shot case. Left for the actual design pass when this gets built.

## Schema and versioning

Protobuf was considered and rejected — it's the obvious "versioned, schema'd" answer, but not a human-readable one, and readability (a plugin author can eyeball a request without tooling) is a real requirement here, not a nice-to-have. **JSON Schema** (the actual IETF/OpenJS spec — a schema is itself a JSON document, not to be confused with JSON-RPC, the envelope shape above) is the fit: git-diffable, human-readable, validated by a library in every language a plugin author would realistically use.

Two independent things need versioning, and conflating them is a mistake:

- **The envelope** (JSON-RPC 2.0 for persistent mode, plain stdin-in/stdout-out for one-shot) — this is the wire protocol itself, expected to change rarely if ever. `"jsonrpc": "2.0"` already pins the spec version; `lucid.plugin/1` is lucid's own protocol-version string layered on top, covering things JSON-RPC doesn't (one-shot's framing, which trait methods exist at all, the manifest/handshake shape below).
- **Per-method payload shapes** — the JSON shape of params/results for each individual trait method (`TrackerAdapter::query_by_decision_state`'s params/result, `NotificationSink::on_needs_review`'s params, etc.). These evolve independently and far more often, as the underlying Rust types they mirror (`TrackerIssue`, `DecisionState`, ...) gain fields. Each method's schema should carry its own version, not share one global number with every other method — otherwise an unrelated `NotificationSink` field addition would force every `TrackerAdapter` plugin to re-declare support for no reason.

**Source of truth stays the Rust types, not a hand-maintained schema file.** [`schemars`](https://github.com/GREsau/schemars) derives a JSON Schema document straight from the same `serde`-derived structs the trait methods already use (`TrackerIssue`, `DecisionState`, etc.) — `#[derive(JsonSchema)]` alongside the existing `#[derive(Serialize, Deserialize)]`. This keeps the published schema from drifting out of sync with what the daemon actually sends/expects, since it's generated from the real type, not written by hand a second time.

**Compatibility rule**, mirroring ordinary semver so it needs no new mental model: a MINOR bump is additive-only (new optional field, safe for a plugin built against an older MINOR to ignore); a MAJOR bump is breaking (renamed/removed/required field, changed type) and requires the plugin to explicitly declare it supports that MAJOR version. lucid refuses to load a plugin declaring a MAJOR version it doesn't recognize — fails loud at load time, never guesses at compatibility.

**Parsing stance (Postel's Law — see [ux-principles](ux-principles.md)): liberal in what the daemon accepts, conservative in what it sends.** Deserializing a plugin's JSON-RPC response tolerates unknown/extra fields rather than hard-failing — the default `serde` behavior, deliberately *not* `#[serde(deny_unknown_fields)]` — consistent with the MINOR-additive rule above: a plugin author adding a field the daemon doesn't know about yet shouldn't break anything. In the other direction, everything the daemon sends to a plugin is strictly schema-valid, compact JSON with no ambiguity — the daemon is the stricter party, since it's the one guaranteed to be running the current version.

**Where a plugin declares its supported version:**

- **One-shot** — a small manifest file discovered next to the script (e.g. `.lucid/tracker/manifest.json` — `{"protocol": "lucid.plugin/1", "schema_versions": {"query_by_decision_state": "1.2"}}`), read once at daemon startup, not on every call.
- **Persistent** — a version string exchanged in the first line sent/received after spawn, before any real request. Deliberately not MCP's `initialize` handshake (no capability negotiation, no dynamic discovery) — just a version check, reject-and-exit if it doesn't match anything the daemon supports.

## Directionality

JSON-RPC 2.0 doesn't assume a fixed client/server role — request, response, and notification are message *types*, not roles pinned to one side of the connection; either peer may originate a request. Over stdio this falls out of the transport for free: stdin and stdout are two independent, unidirectional pipes, so both directions already exist without any extra design — a peer just writes to its own outbound stream when it has something to send, and matches responses against the `id`s it itself generated (no cross-peer id coordination needed, since a response only ever arrives on the inbound stream of whichever peer sent the matching request). LSP already runs exactly this shape in production — the server sends the client real requests (`workspace/configuration`), not just responses.

v1 scope stays daemon-initiates-only (the daemon calls `TrackerAdapter` methods on the plugin; the plugin has no need to push anything back, since lucid is tick/poll-based, not event-driven) — but nothing about the envelope forecloses a plugin initiating a request later (e.g. "something changed externally, re-check now"). That would need a daemon-side read loop that also accepts unsolicited requests, not a protocol change.

## Control, not just observation

A script invoked at a gate/notify point only sees what's serialized to it — it has no in-process handle back into the daemon the way a pi `ExtensionAPI` object does. That's fine for *reacting*; for a script that needs to *act* (approve a different issue, override presence, cancel a dispatch), the answer isn't a bespoke callback protocol — it's that the script shells back out to lucid's own CLI (`lucid task approve`, `lucid presence override`, etc., see `docs/CLI.md`), the same interface a human uses. Most of those act on tracker/config state directly and need no live daemon process. The exceptions — cancelling an in-flight dispatch, anything needing `lucid stop`'s control socket — are bounded by the same not-yet-built cross-process IPC layer noted in `docs/CLI.md` (`lucid stop`) and `docs/FEATURES.md`, not a new gap this design introduces.

## Pilot: `ScriptSink` (`NotificationSink`) — shipped

Not just because it has no typed built-in to conflict with, but because it's the sharpest illustration of the whole point of this design. Notification platforms are exactly where software conventionally over-builds — a first-party `WebhookSink`/`DiscordSink`/`SlackSink` per platform, each only ever half-matching how any one operator actually wants their notifications formatted. A script escape hatch makes that entire category of embedded-Rust work unnecessary: one operator's Discord webhook, curl'd from a five-line script in `.lucid/notify/on_needs_review`, is a complete, correct implementation without lucid's binary knowing Discord exists. `ScriptSink` implements the trait's three methods (`on_awaiting_input`, `on_needs_review`, `on_done`, see [human-in-the-loop](human-in-the-loop.md)) as one-shot invocations per event — low frequency, no case for persistent mode here.

**Built** (`src/notify/`): `NotificationSink` trait + `NullSink` (default) + `ScriptSink`, wired into `worker::finalize_completion`/`mark_done` for `on_needs_review`/`on_done` (all three `NeedsReview`-reaching code paths, both `Done`-reaching paths). `on_awaiting_input` is implemented but has no caller yet — depends on the still-undesigned `NEEDS_INPUT:` marker parsing. Config: `[notifications]` — `backend` (`"null"`/`"script"`), `script_dir` (default `.lucid/notify`), `timeout_secs` (default `10`, matching this page's one-shot default). Payload includes `"protocol": "lucid.plugin/1"` even though the version-negotiation machinery around it isn't built — free to include now, would be a breaking change to add later. Sink failures are always logged and swallowed, never propagated — proven by a dedicated test (`finalize_completion_survives_a_failing_sink`).

**Known limitation, documented rather than silently accepted**: `[notifications]` is one global config section; in multi-project mode every project shares one `script_dir` resolved against `daemon.workdir`, not a per-project path.

## Open questions

- Whether `DispatchPolicy` should be a real new trait or just a second `verify_cmd`-shaped config field (`pre_dispatch_cmd`) — the latter matches existing precedent more closely and needs no new trait machinery.
- Whether concurrent in-flight persistent-mode requests need pipelining (`id`-matched out-of-order responses) or a strict one-at-a-time request/response cycle is sufficient given lucid's actual call volume.
- One-shot script layout (one executable per method vs. one script dispatching on an argv verb) — depends on real usage patterns that don't exist yet.

## Process model: one persistent process per trait instance, not per method

A script-backed `TrackerAdapter` is one long-running process handling all five trait methods (`query_by_decision_state`, `approve`, `attach_note`, `attach_link`, `list_comments`) over its single JSON-RPC connection — routed by the `method` field on each request, the same way any JSON-RPC server multiplexes calls. Not one process per method; that would mean N times the idle processes for no benefit, since the whole point of persistent mode is amortizing one process's startup cost across every call it'll ever receive.

## Persistent-process lifecycle

**Restart/backoff** (resolved): standard exponential-backoff-with-jitter, the systemd/Kubernetes convention rather than anything bespoke — start at ~1s, double each consecutive failure, cap around 30–60s, ±20–30% jitter to avoid synchronized restart storms if multiple script-backed traits fail at once. Crash-loop protection on top: after N consecutive failures inside a rolling window (e.g. 5 within 60s, matching systemd's `StartLimitBurst`/`StartLimitIntervalSec` shape), stop auto-restarting and mark that trait's script backend `Failed` rather than looping forever — surfaced via whatever the daemon's existing error-visibility path is, not a new one.

**Timeouts** (resolved, concrete defaults — all configurable, not hardcoded): these are lightweight local-subprocess calls, not harness dispatches, so they should be judged on a completely different scale than `stall_timeout_secs`' 600s default for a real LLM task:
- Version handshake (first line after spawn, persistent mode): 5s.
- Any individual call — one-shot invocation, or one persistent-mode request/response: 10s.

A call that times out is treated as a failure for that call, feeding the same restart/backoff path above if it's a persistent process (repeated timeouts count toward the crash-loop threshold, not just hard exits).

## Related pages

- [pi-harness-extensibility](../research/pi-harness-extensibility.md) — the research this design responds to
- [tracker-adapter](tracker-adapter.md)
- [presence-detection](presence-detection.md)
- [harness-dispatch](harness-dispatch.md)
- [worker-completion](worker-completion.md) — `verify_cmd`, the existing proof of pattern
- [human-in-the-loop](human-in-the-loop.md) — `NotificationSink`
- [sandboxed-execution](sandboxed-execution.md) — why trust-routing validation stays out of scripting reach
