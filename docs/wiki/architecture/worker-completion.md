# Worker Completion

How a successful dispatch turns into "this task is done" — and who gets to decide that. Two independent axes, both resolved per-issue rather than per-repo: [`CompletionMode`](#completing-the-working-tree) (what happens to the working tree) and [`ReviewMode`](#closing-the-tracker-item) (who signs off before the tracker item closes).

Before this page, neither existed: a "successful" dispatch was just the harness's own `is_error`/exit-code self-report, and nothing ever moved the tracker item out of `proposal:approved` — see [Agent handoff](agent-handoff.md) for the parallel gap that made prompts carry only the issue title, not its description.

## Completing the working tree

`daemon.completion_mode` (`CompletionMode::None` default, or `CompletionMode::Commit`):

- **`None`** — today's original behavior. lucid never touches git; whatever the harness did (or didn't do) to the working tree is left as-is.
- **`Commit`** — `worker::dispatch_prompt` appends an instruction telling the harness to `git commit` its own work once it's confident any `acceptance_criteria` are met — explicitly "one commit or several, whatever's logically right for the change," with the issue id required in at least one commit message, and never to push or open a PR.

There is deliberately no `BranchAndPr` mode and no worktree isolation used here. [Per-issue worktree isolation](../../FEATURES.md) is still unbuilt — dispatch runs directly in the single configured `daemon.workdir`, shared across every issue. That constraint is what rules out lucid running `git add -A && git commit` itself: in a shared tree it can't tell the harness's changes apart from an unrelated in-progress edit or `state/tracker.json`. The harness, by contrast, knows exactly what it changed and can write an honest commit message — so it commits, and lucid only *observes* the result.

Observation is a commit **list**, not a single SHA compare: `worker::commits_since` runs `git log --oneline <head-before-dispatch>..HEAD` and `describe_commit_result` reports every entry ("N commit(s):" plus each one-liner), falling back to `git status --porcelain`'s dirty-file count ("Left N files uncommitted") or "No changes" when nothing landed. Earlier this only compared `HEAD` before/after and reported a single SHA — silently dropping every commit but the last if the harness made more than one. Since a harness making several commits per task turns out to be normal, not an edge case (see "Is direct-commit even the right default?" below), a single-SHA compare would have been actively misleading, not just imprecise.

This also means `Commit` mode requires `daemon.workdir` to actually be a git repository — if it isn't, the observation step reports "Commit status unknown" rather than failing the dispatch.

"PRs are a bottleneck" was an explicit call for the current bootstrap phase (human still approves every proposal before it dispatches) — not a permanent architectural stance. A `BranchAndPr` mode is the natural next addition once worktree isolation exists to make blind `git add -A` safe.

### Is direct-commit-to-shared-directory even the right default?

A `research-first` pass (2026-08-17, in-session) grounded current practice for how autonomous coding agents complete tasks in git, prompted by a direct question about this design. Two findings worth being explicit about rather than just quietly acting on:

- **Commit granularity**: Aider's own docs (<https://aider.chat/docs/git.html>) confirm it auto-commits after *every file edit* by default, specifically so `/undo` can cleanly revert one step — not one commit per task. This is why `dispatch_prompt`'s wording was loosened (above) instead of implying a single commit.
- **Isolation**: several search-result-level sources (a buildmvpfast 2026 git-workflow post, addyosmani.com's "Code Agent Orchestra" synthesis) converged on "never let an agent commit to a shared branch — every task gets its own feature branch/worktree" as a baseline rule for GitHub Copilot coding agent, Cursor background agents, and multi-agent orchestrators generally. These weren't deep-fetched primary sources the way the Aider docs were — treat this as corroborating signal, not a fully verified citation.

Read together: `CompletionMode::Commit`'s shared-`daemon.workdir`, no-worktree, direct-commit design is a real, named anti-pattern in current agent-orchestration practice — not merely a smaller-scale version of branch-plus-PR. It's a deliberate, acknowledged tradeoff for the current phase (a human approves every task before dispatch, which substitutes for PR *review* but not for worktree *isolation* — two dispatches, or a dispatch overlapping your own local edits, can still collide in the same directory), not something the design was unaware of. This raises the practical priority of [per-issue worktree isolation](../../FEATURES.md) from "eventually, to enable `BranchAndPr`" to "the thing this mode is currently missing that current practice treats as load-bearing."

## Closing the tracker item

`Proposal.review` / `TrackerIssue.review` (`ReviewMode`, defaults to `Auto`) — this is the fork [Review/rework UX](review-rework-ux.md) already flagged as "still open," resolved as one field instead of a repo-wide policy so a human (or, in phase two, the PM/research layer) can dial trust up or down per ticket:

- **`Auto`** — a `Succeeded` dispatch moves the issue straight to `DecisionState::Done`. No gate. This is the only mode that existed as a real signal before this page — everything else is new.
- **`Human`** — a `Succeeded` dispatch moves the issue to `DecisionState::NeedsReview` and stops. The dispatch loop never re-picks a `NeedsReview` issue up on its own (it's off the `proposal:approved` label/state entirely) — a human has to look and flip it back to `Approved` (retry) or forward to `Done` themselves. This is the first real implementation of the "parked state" rule `docs/FEATURES.md` had flagged as unbuilt.
- **`Agent`** — first, a deterministic gate: if `Proposal.verify_cmd` is set, `worker::run_verify_cmd` runs it (`sh -c`, in `daemon.workdir`) and checks the exit code *itself* — lucid, not an LLM's summary of whether it passed. A nonzero exit (or the command failing to even run) short-circuits straight to `NeedsReview` with a note; the review dispatch below never runs at all. If it passes (or `verify_cmd` is unset), a second, read-only dispatch (`worker::agent_review`, same profile list, `pm::wake`'s restricted non-mutating `--allowedTools` surface) reviews the pending diff against `acceptance_criteria` and replies with a single `VERDICT: PASS` / `VERDICT: FAIL: <reason>` line. `PASS` → `Done`; `FAIL`, an unparseable reply, or the review dispatch itself erroring all route to `NeedsReview` rather than guessing — fail-open-to-human, never a silent pass.

A `Failed`/`TimedOut` run never reaches this decision at all (`worker::finalize_completion` is a no-op for anything but `Succeeded`) — the issue stays `Approved`, which is exactly the state `daemon::dispatch_approved_issues`'s existing retry check already looks for.

### `verify_cmd`: deterministic, but deliberately optional

Prompted by a direct question during design ("how is this configured — doesn't a required per-repo/per-task setting get tedious?"), `verify_cmd` was built as an *override*, not a requirement:

- **Unset (the common case)**: the review agent is expected to infer its own verification command from the repo's own conventions — `Cargo.toml`, `package.json`, `CLAUDE.md` — the same way a human contributor would figure out how to run the tests. No config step, per-repo or per-task.
- **Set**: pins an exact command, for when auto-inference would pick the wrong thing or you want a specific fast subset rather than a full suite.

This is why the deterministic check only exists for `verify_cmd`'s exit code, not for "did the diff satisfy `acceptance_criteria`" generally — that second judgment stays LLM-based (`agent_review`) regardless, since there's no exit-code equivalent for it. `verify_cmd` closes the part of the gap that has one; it doesn't make the whole `Agent` mode deterministic.

Storage-wise, `verify_cmd` doesn't get the `review` field's treatment (a Linear label lucid parses back structurally) — it's free text, which can't be a label. Instead it's read back via `tracker::frontmatter_field`, which parses one scalar key out of the same frontmatter block `render_description` already renders. This is a deliberate, narrow exception to the pattern [Agent handoff](agent-handoff.md) otherwise documents (frontmatter is for the *harness* to read as prose, not for lucid's own code to re-parse) — justified here because, unlike `review`, there was no label-shaped alternative.

**Live-tested, both branches, against a real `claude -p` dispatch** (not just unit tests): a failing `verify_cmd` correctly short-circuited before the review dispatch ever ran (confirmed by passing empty harness profiles — if the review path had been reached, it would have hit `DispatchError::NoProfiles` and returned `Err`, but the call returned `Ok`). A passing `verify_cmd` correctly proceeded to a real review dispatch. That live run also surfaced a real, worth-knowing characteristic: the reviewer's restricted `--allowedTools` (`Bash(git diff *)`/`log *`/`status *` only) can `permission_denied` a compound Bash command the model tries (e.g. a piped command), which can end the reviewer's turn without a parseable `VERDICT:` line. That correctly failed open to `NeedsReview`, not a false `Done` — a manual rerun of the identical prompt succeeded cleanly on the next attempt, consistent with ordinary LLM non-determinism rather than a bug in this logic. Not a documented guarantee that this never happens — an observed characteristic of `ReviewMode::Agent` worth expecting occasionally.

## Triggering this on demand

`worker::dispatch_and_finalize` bundles "build the prompt, dispatch, finalize per `ReviewMode`" into one call — `daemon::dispatch_approved_issues` uses it on its regular presence-gated tick, and `lucid task dispatch-now <issue-id>` (see `docs/CLI.md`) calls the *identical* function on demand for one issue, instead of waiting for the next tick. It requires the issue already be `Approved` in the tracker and refuses otherwise: the CLI command changes **when** approved work runs, it is never a second authority deciding **whether** work is allowed to run — that decision stays with whatever set the tracker's decision state (a human in Linear, or `lucid task approve`, which itself is just another caller of the same `TrackerAdapter::set_decision_state` the daemon reads back).

## What still verifies nothing

`ReviewMode::Auto`/`Human` still trust the harness's own `is_error` self-report completely — no test run, no build check, no diff-vs-`acceptance_criteria` comparison, regardless of `verify_cmd`. Only `Agent` mode runs anything: `acceptance_criteria` (present in the frontmatter since [Agent handoff](agent-handoff.md)) is first read by `verify_cmd`'s deterministic exit-code check when set, then always by `agent_review`'s LLM judgment of the diff. That LLM judgment itself is still unverified in the sense that matters — a research pass into current agent-completion-signaling practice (2026-08-17, in-session) found that current tools favor deterministic CI/test gates plus PR review over trusting a model's self-report, which is exactly what motivated building `verify_cmd` rather than leaving `Agent` mode as pure LLM-judges-LLM. What `verify_cmd` doesn't close: whether the *diff itself* actually satisfies `acceptance_criteria` beyond "the tests pass" — a green test suite doesn't prove the acceptance criteria were met, only that nothing broke.

## Setup requirement (Linear backend)

Same pattern as the existing `proposal:*` decision labels: `LinearAdapter::label_id` looks labels up, it never creates them. A Linear workspace using this backend needs `review:auto`, `review:human`, and `review:agent` labels created ahead of time, or `create_proposal` fails outright for any proposal with a non-default review mode.
