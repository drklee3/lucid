# lucid — project instructions

This is the schema layer of the repo's [LLM Wiki](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f) (see `docs/wiki/index.md`). It defines two things: how code comments relate to the wiki, and how the wiki itself is maintained.

## Code comments

Default to no comments. Only add one when the *why* isn't obvious from the code itself — a non-obvious constraint, a workaround for a specific bug, a deliberate trade-off, a surprising invariant. Never explain *what* the code does — good naming already does that. Keep them short: one line, maybe two. If a comment needs a paragraph, the code likely needs restructuring, or the rationale belongs in the wiki (below), not the comment.

## Deep rationale goes in the wiki, not the comment

This repo keeps `docs/wiki/` for exactly this reason. When a piece of code needs more than 1-2 lines to explain (a design decision, a verified/unverified fact, a bug found by testing, a tradeoff), the rationale belongs in the relevant `docs/wiki/` page, not inline. The comment stays short and points there.

**Before**: a 10+ line comment block re-explaining a whole decision.

**After**: 1-2 lines stating the constraint, plus a link.

```rust
// IdleHint never resets under WSL2 — no seat/HID backend. See
// docs/wiki/architecture/presence-detection.md.
fn logind_is_idle(&self) -> bool { ... }
```

If the rationale doesn't have a wiki page yet, add one (or a section to an existing page) rather than inlining it in the comment.

## Wiki operating rules

The wiki (`docs/wiki/`) is the source of truth for this project's design — not a derived layer sitting on top of some other authoritative document. It started from an initial design/research brainstorming pass (ingested 2026-08-16, see `git log` for that content if ever needed — it wasn't kept as a live file since it was internal brainstorming, not an external reference worth preserving in the tree). Since then, the wiki is maintained directly: pages get edited in place as understanding changes, not re-derived from a frozen source each time.

- **One page per concept.** Don't let one page sprawl to cover several distinct ideas — split when a topic grows large enough to stand alone.
- **Update `docs/wiki/index.md` on every page add/remove** — one line per page, one-line summary, grouped by category (currently `architecture/` and `research/`).
- **`docs/wiki/log.md` is append-only.** Never edit or delete a past entry — correct forward with a new entry. Every entry uses the exact prefix `## [YYYY-MM-DD] <op> | <title>`, where `<op>` is `ingest` (a new source was decomposed into the wiki) or `query` (a synthesized answer was filed back as a new/updated page) — keep it machine-parseable.
- **Good answers to ad-hoc questions get filed back into the wiki** as new pages (or additions to existing ones), not left to disappear into chat history. Log the query with the `query` prefix.
- **Run a lint pass periodically** (and always right after a large update): check for contradictions between pages, stale claims (a page asserting something a later decision contradicts), orphan pages (not linked from `index.md` or any other page), missing cross-references, and data gaps. Fix what you find; don't just report it.
- **Cite real external sources inline** when a claim actually depends on one (a blog post, an incident writeup, a library's docs) — a `Source:` line or inline link, next to the specific claim it backs. Don't manufacture citations for internal reasoning that has no external source; the wiki page itself is the record for that.
