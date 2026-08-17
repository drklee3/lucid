# Trace Correlation

## The problem

Once the Worker dispatches autonomously and nobody's watching live, "why did this run go wrong" has to be answerable after the fact from stored data, not from having watched the terminal. Both supported harnesses (Claude Code, Codex — see [harness dispatch](harness-dispatch.md)) can emit OpenTelemetry traces of their own execution (tool calls, model calls, prompts), but a raw trace store on its own only answers "what happened in *some* run" — not "what happened in *the* run behind ticket X."

## Mechanism: `OTEL_RESOURCE_ATTRIBUTES`, not scraping the harness's own trace ID

`OTEL_RESOURCE_ATTRIBUTES` is a standard OpenTelemetry SDK environment variable (part of the OTel spec itself, not a Claude-Code- or Codex-specific flag) — any spec-compliant SDK merges it into every span's resource attributes automatically. The orchestrator sets it at spawn time on the harness subprocess:

```rust
Command::new("claude")
    .arg("-p")
    .env("CLAUDE_CODE_ENABLE_TELEMETRY", "1")
    .env("OTEL_LOGS_EXPORTER", "otlp")
    .env("OTEL_EXPORTER_OTLP_ENDPOINT", "http://localhost:4317")
    .env("OTEL_RESOURCE_ATTRIBUTES", format!("lucid.ticket_id={ticket_id},lucid.dispatch_id={dispatch_id}"))
    .spawn()?;
```

This is deliberately harness-agnostic: it works for any future harness added to the [harness dispatch](harness-dispatch.md) profile list without new per-harness code, the same way the profile list itself is designed to grow without a rewrite. Codex is the one asymmetric case — its OTEL config is TOML-driven (`~/.codex/config.toml`, user-level only; project-local `otel.*` keys are ignored by Codex itself), so it doesn't pick up `OTEL_RESOURCE_ATTRIBUTES` directly. Either set Codex's OTLP endpoint once globally and skip per-dispatch tagging, or pass the correlation IDs via `-c otel.resource_attributes=...`-style CLI override at spawn if per-dispatch tagging via Codex is needed later.

## No new IDs — reuses state the orchestrator already keeps

- `ticket_id` — already owned by the [tracker adapter](tracker-adapter.md) (e.g. the Linear issue ID).
- `dispatch_id` — already generated per attempt by the orchestration state machine (`Claimed → Running → Released`, kept separate from tracker state — see [Symphony patterns](symphony-patterns.md)). Retries get a fresh `dispatch_id`, so a stall-detect-and-retry cycle produces two distinguishable trace links instead of one polluted trace.

No new correlation ID needs to be invented or persisted anywhere new; this only requires threading two already-known values into an env var at the one place the harness subprocess is spawned.

## Writing the link back: a proof-of-work artifact, not a dashboard feature

Per the [proof-of-work artifacts](observability.md#proof-of-work-artifacts) gap already noted, have the Worker post a trace-query link back to the tracker item via the tracker adapter (same mediation boundary as every other tracker write — see [harness/tracker isolation](harness-tracker-isolation.md), the harness itself never touches the tracker) on dispatch completion, e.g.:

```
http://localhost:6006/projects/{project_id}?filter=lucid.dispatch_id=='{dispatch_id}'
```

Opening the ticket and clicking through to the exact span tree for that run replaces grep-ing terminal output or re-running the task to reproduce a failure.

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

Source: filed from a design conversation extending the [proof-of-work artifacts](observability.md#proof-of-work-artifacts) gap, after confirming both harnesses' built-in OpenTelemetry support.
