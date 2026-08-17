# Agent Handoff Surface

Same frontmatter-body split as Symphony's WORKFLOW.md (see [Symphony patterns](symphony-patterns.md)), embedded directly in the tracker issue: `task_type`, `target_paths`, `acceptance_criteria` (list), `research_ref` (link to the [Research agent](research-agent.md)'s findings, not re-summarized prose).

The Worker parses this deterministically instead of inferring intent from freeform issue text — this is the single biggest lever for avoiding the "worker misinterprets vague proposal" failure mode.

Source: `docs/design.md` resolved decision #4.
