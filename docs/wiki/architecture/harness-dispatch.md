# Harness Dispatch

## A prioritized list of (harness, auth mode) profiles, subscription-first

Both Claude Code and Codex CLI support the same dual-auth pattern in headless mode:

- **Claude Code**: plain `claude -p` reads the existing subscription OAuth login, same as interactive. `--bare` mode explicitly refuses to read OAuth credentials at all and requires `ANTHROPIC_API_KEY` instead.
- **Codex**: `codex exec` reads a ChatGPT Plus/Pro/Business login by default, and flips to metered billing the instant `OPENAI_API_KEY` is set in the environment (overriding the logged-in session automatically).

Given both are already-paid-for subscriptions, the design defaults to subscription auth and treats API-key billing as the *fallback*, not the default — this inverted an earlier draft that defaulted to `--bare`+API-key preemptively.

Harness dispatch is therefore a small ordered list of profiles, not a single hardcoded command:

```
[ {name: "claude-subscription", cmd: "claude -p ...",        auth: subscription, priority: 1},
  {name: "claude-api",          cmd: "claude --bare -p ...", auth: ANTHROPIC_API_KEY, priority: 2},
  {name: "codex-subscription",  cmd: "codex exec ...",       auth: subscription, priority: 1},
  {name: "codex-api",           cmd: "codex exec ...",       auth: OPENAI_API_KEY, priority: 2} ]
```

The orchestrator tries the assigned harness's subscription profile first. On a **detected block** — not any nonzero exit, a specific signal (Claude Code's stream carries typed retry-error categories like `rate_limit`/`billing_error`/`oauth_org_not_allowed` in its `system/api_retry` events; Codex has an analogous error surface) — it falls through to the next profile in priority order: same harness's API-key variant, or a different harness entirely. Same list-with-fallback shape as the [tracker adapter](tracker-adapter.md) and [presence sources](presence-detection.md), applied to harness+auth jointly.

## ToS: resolved, not just assumed

The one real restriction found during research — "Anthropic does not allow third party developers to offer claude.ai login... for their products, including agents built on the Claude Agent SDK" — is about products serving *other* users through your infrastructure. It doesn't apply here: lucid uses the actual `claude` CLI binary, logged in as the account owner, driving that owner's own subscription for personal automation — not the Agent SDK library, not a product offered to third parties. Compliant, not a gray area.

## The real tradeoff isn't compliance, it's shared capacity

Subscription usage draws from the same rolling 5-hour/weekly window as interactive daytime use — an autonomous Worker running overnight competes with tomorrow's interactive session for the same pool. API-key billing is metered but doesn't compete at all. That's the actual reason to keep the fallback live.

## Forward-looking, not urgent

Anthropic has stated `--bare` (API-key-only) "will become the default for `-p` in a future release" — not yet, but the fallback-list design means that flip is a priority reordering when it happens, not a rewrite.

## Zero direct tracker access

See [harness/tracker isolation](harness-tracker-isolation.md) — none of these profiles ever get a Linear (or any tracker) credential wired in, regardless of which one is dispatched.
