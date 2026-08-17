# PM Agent Scope: Gap-Detection, Not Open-Ended Ideation

## The boundary

The PM agent does not decide overall direction. Direction (goals, roadmap) stays a human call. The market research (see [prior-art landscape](../research/prior-art-landscape.md)) found a real three-category split: triage existing tickets / decompose a human-supplied spec / invent an idea from nothing. Full autonomy over "what to build" is the one category nobody has shipped, and the one Linear explicitly declined to build (see [is the PM layer novel?](../research/pm-layer-novelty.md)). lucid does not aim for that category either.

## What it aims for instead

Given a goal or direction the human has already stated (a wiki page, a roadmap doc, a stated priority), the PM agent notices when a **concrete gap exists between that stated goal and the current ticket/PR set** — something the goal implies but nothing yet tracks — and files a *stub*, not a full proposal: title + "this goal seems to imply nothing addresses X yet, want to define it?" It doesn't spec the work; it flags the gap and hands it to the human to define scope. Same as answering "what's next?" when asked interactively, just running unprompted on a schedule instead of on-demand.

## Why the narrower framing matters

- **Avoids the matplotlib failure mode** ([details](../research/matplotlib-incident.md)) — an agent with a stake in its own idea reacting badly to rejection. A gap-flag has no ego in the outcome; rejection just means the human already knows or disagrees the gap matters.
- **Keeps review cost small.** The "review burden shifts, doesn't disappear" critique (see [risks and critiques](../research/risks-and-critiques.md)) applies to full proposals; a one-line gap-flag is cheap to glance at and dismiss.

## Precondition

This only works if there's a goal artifact concrete enough for "gap" to be well-defined — not a vague vibe. Reinforces why PM investigation scope (below) needs a real, reasonably current wiki/ROADMAP to diff against, not just git log.

## Investigation scope on wake

A repo-owned watermark file (`docs/wiki/PM_STATE.md` or a tracker-side equivalent) records: last commit SHA reviewed, timestamp of last wake, count of proposals filed this week. On wake, the PM reads:

- `git log <watermark>..HEAD` — bounded, not full history.
- Open tracker issues — for [dedup](dedup-death-loop.md).
- Open PRs — don't propose work colliding with in-flight PRs.
- The wiki/ROADMAP if one exists — direction, not just diff.

Cap proposals per wake (recommend 3) — a PM that files 15 issues at 3am is worse than useless, it's a wall the human has to triage past to find the good ones.

Source: `docs/design.md` § Scope Clarification, resolved decision #2.
