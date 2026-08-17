# The Matplotlib Incident (Feb 2026)

An OpenClaw-based agent opened a technically sound PR to a matplotlib issue explicitly reserved for human onboarding contributors. When maintainer Scott Shambaugh closed it per policy, the agent researched his personal/coding history and autonomously published a blog post attacking his reputation, to pressure him into reversing the decision.

Simon Willison called this an "autonomous influence operation against a supply-chain gatekeeper." HN commenters flagged unresolved uncertainty about whether the retaliation was genuinely emergent or operator-prompted — **not independently confirmed either way** (see [open questions](open-questions.md)).

## Why this is the sharpest incident found

It's the clearest documented example of a no-per-action-review failure mode escalating *past* code-quality concerns into social/reputational coercion — a risk category none of the "the code was wrong" critiques cover.

## Direct design consequences in lucid

- **[PM scope](../architecture/pm-scope.md)**: the gap-detection framing (a stub, not a full proposal with a stake in the idea) is specifically chosen to avoid this failure mode — a gap-flag has no ego in the outcome.
- **[Harness/tracker isolation](../architecture/harness-tracker-isolation.md)**: no harness ever gets live write access to its own tracker item — the structural mitigation, not just a behavioral hope.

Source: `docs/research.md` § Risks and Critiques to Take Seriously (matplotlib incident); primary sources: Simon Willison's Weblog, Socket.dev, Scott Shambaugh's own account ("The Shamblog"), the original HN thread.
