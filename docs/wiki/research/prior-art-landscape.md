# Prior-Art Landscape

## Summary table

| System | Trigger | Input | Output | Proactive capability? |
|---|---|---|---|---|
| OpenAI Symphony | Tracker issue assigned/open | Issue text, repo | PR, CI status, conflict resolution | No — pre-existing issues only |
| Linear Coding Sessions | Manual delegation, @-mention, triage rule | Issue text, repo, skills.md | PR, review threads, verification artifacts | No — entirely reactive |
| Factory Missions / Droids | Ticket queue pull | Task decomposition into parallel tracks | Code, review, docs, test outputs | No |
| Cursor Background Agents | Manual/GitHub issue (immature) | Repo clone into cloud sandbox | PR (unreliable auto-attribution) | No |
| GitHub Copilot Coding Agent | Issue assigned to Copilot | Issue text, repo, instructions, images | PR with commits, tests, checklist | No — single-issue-triggered |
| cyrus | Issue assignment (Linear/GitHub/GitLab/Slack) | Issue, per-issue git worktree | PR + streamed comments; Orchestrator mode splits epics | Sub-issue creation originates from an existing parent, not unprompted |
| OpenHands (OpenDevin) | Schedule, webhook, issue event | Sandboxed Docker env | Code, tests, PR (~72% SWE-Bench) | No |
| SWE-Agent | Benchmark/task harness | Shell/editor/test-runner interface | Patches | No |
| MetaSwarm | Human invokes skill | Issue → 18 subagents | Merged PR via spec-driven TDD | No — local, human-invoked |
| RunMaestro | Human-launched session | Existing agent CLIs | Orchestrated multi-agent sessions | No — orchestration/UI layer only |
| Devin (Cognition) | Assigned task | Codebase exploration, editable plan | PR; sometimes fixes issues beyond ask | Scope-expansion within an assigned task, not ticket-less |
| Devin Outposts | Same as Devin | Split control/execution plane | Same as Devin | No |
| aeon, background-agents, gh-aw, open-swe, remote-swe-agents, run-gemini-cli, sortie, Contrabass | Webhook/Slack/GitHub-comment/tracker | Varies | Draft/attributed PRs | No documented proactive mode (community-curated list, unverified scale claims) |
| **GitAgents** | Runs after every push (Bug Detection); ongoing analysis (Feature Suggestion) | Codebase patterns, best practices | Filed GitHub issues, suggested features | **Yes — only clear category-(c) match found**, but small, unverified vendor |
| Revo.pm | Jira/Slack/GitHub/Confluence signals | Feedback clusters, competitor signals | Drafted specs/roadmap flags | Markets "autonomous" but is synthesis of human-generated signals |
| Height 2.0 (discontinued Sept 2025) | Bug reports, Slack, spec drift | Existing signals | Triage, dedup, spec updates | No — defunct |
| Linear Agent / Code Intelligence / Automations | Human question or rule | Assigned issues | Answers, flags | No — Linear frames "what to build" as human-only by design |
| Backlog.md | Human-authored idea | Markdown decomposition | Structured tasks + acceptance criteria | No — decomposes an existing idea |
| ChatPRD | Human-initiated conversation | Brainstorm prompts | Specs, roadmap drafts | No — human supplies the seed |
| Gravity | Existing Jira backlog | Stale/duplicate/missing-estimate tickets | Flags for PM review | No — hygiene only |
| AI Hero "triage" skill | Existing messy backlog | Tickets | Classified/deduped tickets | No |
| Sweep | — | Codebase | Self-opened fix PRs | Lightweight proactive-bugfix pattern, adjacent but not tracker-orchestration |
| Greptile / CodeRabbit | Existing PR | PR diff | Review comments (human merges) | No — review-only |

## What this landscape means for lucid's design

This same survey is what grounds the [Symphony patterns](../architecture/symphony-patterns.md) borrowed directly (Symphony is the closest analog to the Worker half) and the [state-machine gap analysis](../architecture/state-machine-gaps.md) (Linear Coding Sessions, cyrus, Copilot, Factory, OpenHands, Cursor compared against lucid's design specifically on state machine, review/rework UX, observability, and error visibility).

See [Is the PM layer genuinely novel?](pm-layer-novelty.md) for the verdict this table feeds into.

Source: `docs/design.md` § Existing Systems Research; `docs/research.md` § Landscape Summary.
