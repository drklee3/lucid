# Wiki Log

Append-only. Do not edit or delete past entries — correct forward with a new entry instead. Format: `## [YYYY-MM-DD] <op> | <title>`, where `<op>` is `ingest` (a new raw source was decomposed into the wiki) or `query` (a synthesized answer was filed back as a new/updated page).

## [2026-08-16] ingest | docs/design.md — Autonomous Agentic Development System: Brainstorming & Research

Decomposed into 15 pages under `architecture/`: overview, tech-stack, presence-detection, tracker-adapter, agent-handoff, harness-dispatch, harness-tracker-isolation, pm-scope, research-agent, dedup-death-loop, symphony-patterns, state-machine-gaps, review-rework-ux, observability, error-stall-visibility. The "Next steps" checklist section was deliberately left un-ingested (see `index.md` § Not yet ingested) — it's a living TODO, not stable knowledge.

## [2026-08-16] ingest | docs/research.md — Autonomous Coding-Agent Orchestration and the Proactive-PM Question: Research Findings

Decomposed into 7 pages under `research/`: prior-art-landscape, pm-layer-novelty, practitioner-reality, risks-and-critiques, matplotlib-incident (split out as its own page — cross-referenced from multiple architecture pages, warranted standalone treatment), presence-automation-prior-art, open-questions. The full sources list (~70 citations) was not split into a dedicated `sources.md` — each page cites its own load-bearing sources inline instead; the raw `docs/research.md` § Sources remains the exhaustive citation list if a full audit is ever needed.

## [2026-08-16] query | design.md's un-ingested "Next steps" checklist was stale

The `lucid` repo's initial scaffold (`docs/FEATURES.md`, `docs/CLI.md`, the Rust skeleton) resolved or superseded most of `docs/design.md`'s "Next steps" list without that section ever being updated to say so. Corrected the section in place (marked done/superseded/still-open per item) and pointed future maintenance at `docs/FEATURES.md` § Deferred / not v1 instead, to stop two open-items lists drifting apart. This is the exception the "immutable raw source" rule already carved out — that section was deliberately excluded from ingestion for exactly this reason (it churns faster than wiki pages should); editing it is consistent with that, not a violation of it. No wiki pages needed changes as a result — the correction was scoped to that one section of the raw source.

## [2026-08-16] query | Rust toolchain/dependency currency audit; `PresenceSource` made async

Full dep audit (all 12 pinned crates confirmed at registry-latest via `cargo info` + `cargo update --dry-run`; none stale or hallucinated). Applied: `[lints]` table (`unsafe_code = "forbid"`, `clippy::pedantic = "warn"`) and `rust-version = "1.87"` (zbus's MSRV, the highest floor among direct deps) added to `Cargo.toml`; `reqwest` features widened to keep `http2`/`system-proxy` after disabling default-features for the rustls swap; `WorkerRun.last_event_at` changed from `std::time::Instant` to `chrono::DateTime<Utc>` (`Instant` can't serialize and isn't wall-clock, and this field is headed for SQLite). Design question surfaced and resolved with the user: `PresenceSource` was sync-signature over an inherently-async D-Bus read; changed to `async fn` (`async-trait`, matching `TrackerAdapter`'s existing pattern) rather than a sync-with-background-cache shim — rationale filed to `architecture/presence-detection.md`.
