# Trace Correlation

## The problem

Once the Worker dispatches autonomously and nobody's watching live, "why did this run go wrong" has to be answerable after the fact from stored data, not from having watched the terminal. Both supported harnesses (Claude Code, Codex — see [harness dispatch](harness-dispatch.md)) can emit OpenTelemetry traces of their own execution (tool calls, model calls, prompts), but a raw trace store on its own only answers "what happened in *some* run" — not "what happened in *the* run behind ticket X."

## Mechanism: `OTEL_RESOURCE_ATTRIBUTES`, not scraping the harness's own trace ID

`OTEL_RESOURCE_ATTRIBUTES` is a standard OpenTelemetry SDK environment variable (part of the OTel spec itself, not a Claude-Code- or Codex-specific flag) — any spec-compliant SDK merges it into every span's resource attributes automatically. `harness::apply_telemetry` sets it at spawn time on the harness subprocess (current code, `src/harness/mod.rs`):

```rust
cmd.env("CLAUDE_CODE_ENABLE_TELEMETRY", "1")
    .env("CLAUDE_CODE_ENHANCED_TELEMETRY_BETA", "1")
    .env("OTEL_TRACES_EXPORTER", "otlp")
    .env("OTEL_LOGS_EXPORTER", "otlp")
    .env("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc")
    .env("OTEL_EXPORTER_OTLP_ENDPOINT", &telemetry.otlp_endpoint)
    .env("OTEL_RESOURCE_ATTRIBUTES", format!("lucid.ticket_id={ticket_id},lucid.dispatch_id={dispatch_id}"));
```

**`CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1` and `OTEL_TRACES_EXPORTER=otlp` are both required for spans specifically** — this was missing for a while and is the one thing on this page worth flagging as a lesson, not just a fact. `CLAUDE_CODE_ENABLE_TELEMETRY=1` + `OTEL_LOGS_EXPORTER=otlp` alone only exports Claude Code's OTel *logs* signal; distributed tracing (spans) is a separate, off-by-default beta signal per [Claude Code's own docs](https://code.claude.com/docs/en/monitoring-usage). With only the logs exporter set, every dispatch still posted a `trace_link` to the tracker (below) that looked correct but pointed at a project that had never received that dispatch's spans — the whole mechanism this page describes was designed and wired up, but not actually exporting anything to trace on, until both vars above were added. Verified live after the fix: a real dispatch produced spans in Phoenix whose `lucid` resource attribute (`{dispatch_id: ..., ticket_id: ...}`) exactly matched the `dispatch_id` in the tracker note — confirmed by querying Phoenix's SQLite store directly (`docker exec <container> python3 -c "import sqlite3; ..."` against `/mnt/data/phoenix.db`), not just by trusting the UI.

This is deliberately harness-agnostic: it works for any future harness added to the [harness dispatch](harness-dispatch.md) profile list without new per-harness code, the same way the profile list itself is designed to grow without a rewrite. Codex is the one asymmetric case — its OTEL config is TOML-driven (`~/.codex/config.toml`, user-level only; project-local `otel.*` keys are ignored by Codex itself), so it doesn't pick up `OTEL_RESOURCE_ATTRIBUTES` directly. Either set Codex's OTLP endpoint once globally and skip per-dispatch tagging, or pass the correlation IDs via `-c otel.resource_attributes=...`-style CLI override at spawn if per-dispatch tagging via Codex is needed later. Codex's traces-export completeness hasn't been re-verified the way Claude Code's just was — treat it as designed-not-confirmed until it is.

## No new IDs — reuses state the orchestrator already keeps

- `ticket_id` — already owned by the [tracker adapter](tracker-adapter.md) (e.g. the Linear issue ID).
- `dispatch_id` — already generated per attempt by the orchestration state machine (`Claimed → Running → Released`, kept separate from tracker state — see [Symphony patterns](symphony-patterns.md)). Retries get a fresh `dispatch_id`, so a stall-detect-and-retry cycle produces two distinguishable trace links instead of one polluted trace.

No new correlation ID needs to be invented or persisted anywhere new; this only requires threading two already-known values into an env var at the one place the harness subprocess is spawned.

## Writing the link back: a structured attachment, not comment text

Per the [proof-of-work artifacts](observability.md#proof-of-work-artifacts) gap already noted, the Worker posts a trace-query link back to the tracker item via `TrackerAdapter::attach_link(issue_id, "Trace", url)` on dispatch completion, e.g.:

```
http://localhost:6006/projects/{project_id}?filter=lucid.dispatch_id=='{dispatch_id}'
```

This goes through the same mediation boundary as every other tracker write — see [harness/tracker isolation](harness-tracker-isolation.md), the harness itself never touches the tracker — but as a distinct method from `attach_note`. `attach_link` posts a structured title+url attachment (Linear's real `attachmentCreate` mutation for `LinearAdapter`; a labeled entry appended to `notes` for `FileTracker`, which has no structured-attachment concept), not a line embedded in a plain-text note. The dispatch-status note posted alongside it no longer repeats the link as text — see [tracker adapter](tracker-adapter.md#structured-attachments-attach_link-vs-attach_note) for the trait-level distinction. Opening the ticket and clicking through to the exact span tree for that run replaces grep-ing terminal output or re-running the task to reproduce a failure.

## Local trace backend: OSS, single container, SQLite by default

[Arize Phoenix](https://arize.com/docs/phoenix) is OTLP-native (real OTLP gRPC/HTTP receiver, not a reimplementation of the wire protocol) with its own storage underneath (SQLite by default, Postgres/ClickHouse are opt-in scaling paths — not needed at this system's scale). It also normalizes incoming spans across semantic-convention dialects (OpenInference vs. the OTel GenAI conventions), which matters here because Claude Code and Codex don't emit under a shared attribute vocabulary (`claude_code.*` vs. `codex.*` prefixes) — Phoenix's translation layer absorbs that instead of requiring a hand-rolled OTel Collector transform. A single Phoenix container is enough; a Collector is only worth adding later if a second backend or custom attribute rewriting is actually needed, not preemptively.

```yaml
services:
  phoenix:
    image: arizephoenix/phoenix:latest
    ports:
      - "6006:6006"   # UI + OTLP/HTTP (/v1/traces)
      - "4317:4317"   # OTLP/gRPC
    volumes:
      - phoenix-data:/mnt/data
    environment:
      - PHOENIX_WORKING_DIR=/mnt/data
volumes:
  phoenix-data:
```

Runs as its own container; the `lucid` binary itself stays undockerized (see [presence detection](presence-detection.md) — it needs direct host access for idle signals and git worktrees, which a container would work against, not help).

**Gotcha:** a stray host-level Phoenix process (e.g. `uv tool run arize-phoenix serve --port 6006`, started outside Docker for unrelated work) can silently occupy `localhost:6006`/`4317` before the container starts. Docker doesn't reliably fail loudly in that case — `docker compose up -d` can report success while `localhost` continues resolving to the other process's own (unrelated) project data instead of this container's. If `/v1/projects` ever shows projects this repo never created, check `ps aux | grep phoenix` for a stray process before assuming anything about lucid's own telemetry is wrong; kill it and `docker compose restart phoenix` to make the container's own port binding take effect.

Source: filed from a design conversation extending the [proof-of-work artifacts](observability.md#proof-of-work-artifacts) gap, after confirming both harnesses' built-in OpenTelemetry support.
