# Wiki Log

Append-only. Do not edit or delete past entries — correct forward with a new entry instead. Format: `## [YYYY-MM-DD] <op> | <title>`, where `<op>` is `ingest` (a new raw source was decomposed into the wiki) or `query` (a synthesized answer was filed back as a new/updated page).

## [2026-08-16] ingest | docs/design.md — Autonomous Agentic Development System: Brainstorming & Research

Decomposed into 15 pages under `architecture/`: overview, tech-stack, presence-detection, tracker-adapter, agent-handoff, harness-dispatch, harness-tracker-isolation, pm-scope, research-agent, dedup-death-loop, symphony-patterns, state-machine-gaps, review-rework-ux, observability, error-stall-visibility. The "Next steps" checklist section was deliberately left un-ingested (see `index.md` § Not yet ingested) — it's a living TODO, not stable knowledge.

## [2026-08-16] ingest | docs/research.md — Autonomous Coding-Agent Orchestration and the Proactive-PM Question: Research Findings

Decomposed into 7 pages under `research/`: prior-art-landscape, pm-layer-novelty, practitioner-reality, risks-and-critiques, matplotlib-incident (split out as its own page — cross-referenced from multiple architecture pages, warranted standalone treatment), presence-automation-prior-art, open-questions. The full sources list (~70 citations) was not split into a dedicated `sources.md` — each page cites its own load-bearing sources inline instead; the raw `docs/research.md` § Sources remains the exhaustive citation list if a full audit is ever needed.
