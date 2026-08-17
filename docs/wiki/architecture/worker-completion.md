# Worker Completion

How a successful dispatch turns into "this task is done" — and who gets to decide that. Two independent axes, both resolved per-issue rather than per-repo: [`CompletionMode`](#completing-the-working-tree) (what happens to the working tree) and [`ReviewMode`](#closing-the-tracker-item) (who signs off before the tracker item closes).

Before this page, neither existed: a "successful" dispatch was just the harness's own `is_error`/exit-code self-report, and nothing ever moved the tracker item out of `proposal:approved` — see [Agent handoff](agent-handoff.md) for the parallel gap that made prompts carry only the issue title, not its description.

## Completing the working tree

`daemon.completion_mode` (`CompletionMode::None` default, or `CompletionMode::Commit`):

- **`None`** — today's original behavior. lucid never touches git; whatever the harness did (or didn't do) to the working tree is left as-is.
- **`Commit`** — `worker::dispatch_prompt` appends an instruction telling the harness to `git commit` its own work once it's confident any `acceptance_criteria` are met, with the issue id in the commit message, and never to push or open a PR.

There is deliberately no `BranchAndPr` mode and no worktree isolation used here. [Per-issue worktree isolation](../../FEATURES.md) is still unbuilt — dispatch runs directly in the single configured `daemon.workdir`, shared across every issue. That constraint is what rules out lucid running `git add -A && git commit` itself: in a shared tree it can't tell the harness's changes apart from an unrelated in-progress edit or `state/tracker.json`. The harness, by contrast, knows exactly what it changed and can write an honest commit message — so it commits, and lucid only *observes* the result (`git rev-parse HEAD` before/after the dispatch, `git status --porcelain` after) and reports "Committed `<sha>`" / "Left N files uncommitted" / "No changes" in the same tracker note the trace link goes into.

This also means `Commit` mode requires `daemon.workdir` to actually be a git repository — if it isn't, the observation step reports "Commit status unknown" rather than failing the dispatch.

"PRs are a bottleneck" was an explicit call for the current bootstrap phase (human still approves every proposal before it dispatches) — not a permanent architectural stance. A `BranchAndPr` mode is the natural next addition once worktree isolation exists to make blind `git add -A` safe.

## Closing the tracker item

`Proposal.review` / `TrackerIssue.review` (`ReviewMode`, defaults to `Auto`) — this is the fork [Review/rework UX](review-rework-ux.md) already flagged as "still open," resolved as one field instead of a repo-wide policy so a human (or, in phase two, the PM/research layer) can dial trust up or down per ticket:

- **`Auto`** — a `Succeeded` dispatch moves the issue straight to `DecisionState::Done`. No gate. This is the only mode that existed as a real signal before this page — everything else is new.
- **`Human`** — a `Succeeded` dispatch moves the issue to `DecisionState::NeedsReview` and stops. The dispatch loop never re-picks a `NeedsReview` issue up on its own (it's off the `proposal:approved` label/state entirely) — a human has to look and flip it back to `Approved` (retry) or forward to `Done` themselves. This is the first real implementation of the "parked state" rule `docs/FEATURES.md` had flagged as unbuilt.
- **`Agent`** — a `Succeeded` dispatch triggers a second, read-only dispatch (`worker::agent_review`, same profile list, `pm::wake`'s restricted non-mutating `--allowedTools` surface) that reviews the pending diff against `acceptance_criteria` and replies with a single `VERDICT: PASS` / `VERDICT: FAIL: <reason>` line. `PASS` → `Done`; `FAIL`, an unparseable reply, or the review dispatch itself erroring all route to `NeedsReview` rather than guessing — fail-open-to-human, never a silent pass.

A `Failed`/`TimedOut` run never reaches this decision at all (`worker::finalize_completion` is a no-op for anything but `Succeeded`) — the issue stays `Approved`, which is exactly the state `daemon::dispatch_approved_issues`'s existing retry check already looks for.

## Triggering this on demand

`worker::dispatch_and_finalize` bundles "build the prompt, dispatch, finalize per `ReviewMode`" into one call — `daemon::dispatch_approved_issues` uses it on its regular presence-gated tick, and `lucid task dispatch-now <issue-id>` (see `docs/CLI.md`) calls the *identical* function on demand for one issue, instead of waiting for the next tick. It requires the issue already be `Approved` in the tracker and refuses otherwise: the CLI command changes **when** approved work runs, it is never a second authority deciding **whether** work is allowed to run — that decision stays with whatever set the tracker's decision state (a human in Linear, or `lucid task approve`, which itself is just another caller of the same `TrackerAdapter::set_decision_state` the daemon reads back).

## What still verifies nothing

`ReviewMode::Auto`/`Human` still trust the harness's own `is_error` self-report completely — there's no test run, no build check, no diff-vs-`acceptance_criteria` comparison unless `ReviewMode::Agent` is set. `Agent` mode is the first place `acceptance_criteria` (present in the frontmatter since [Agent handoff](agent-handoff.md)) actually gets read by anything, instead of being dead data.

## Setup requirement (Linear backend)

Same pattern as the existing `proposal:*` decision labels: `LinearAdapter::label_id` looks labels up, it never creates them. A Linear workspace using this backend needs `review:auto`, `review:human`, and `review:agent` labels created ahead of time, or `create_proposal` fails outright for any proposal with a non-default review mode.
