# Agent Handoff Surface

Same frontmatter-body split as Symphony's WORKFLOW.md (see [Symphony patterns](symphony-patterns.md)), embedded directly in the tracker issue: `task_type`, `target_paths`, `acceptance_criteria` (list), `research_ref` (link to the [Research agent](research-agent.md)'s findings, not re-summarized prose), `review` (see [Worker completion](worker-completion.md)).

Two different things read this surface, deterministically in different ways:

- **lucid itself** parses `review` deterministically — via a `review:auto`/`review:human`/`review:agent` tracker label (Linear label / `FileTracker` field), not by re-parsing the rendered frontmatter text. `TrackerIssue::review` carries the already-parsed value; `render_description`'s `review:` frontmatter line is there for a human/harness reading the issue, not lucid's own source of truth.
- **The dispatched harness** (`worker::dispatch_prompt`) receives `task_type`/`target_paths`/`acceptance_criteria`/`research_ref` as the rendered frontmatter+body block appended to the prompt — an LLM reading structured text, not lucid parsing it back out. This is still the intended lever against "worker misinterprets vague proposal": a Worker is handed the same explicit fields every time instead of inferring intent from freeform prose, it just does so by reading them, not by lucid extracting them as data before dispatch.
