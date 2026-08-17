# Research Agent

Validates a proposed task before the [PM agent](pm-scope.md) files it. Tiered depth, not fixed:

- **Default (cheap)**: dependency/API existence check, grep for prior art in-repo, lint against repo conventions (CLAUDE.md, STYLE.md if present). One shallow pass, cheap model.
- **Deep (expensive, opt-in by PM tag)**: a throwaway-workspace prototype spike, only for proposals the PM itself flags as high-uncertainty or high-blast-radius (schema/migration changes, anything security-sensitive, anything touching auth).

The Research agent returns a confidence score alongside findings. The PM has a filing threshold below which it discards rather than files — logged to the [rejected-ideas list](dedup-death-loop.md) either way, so it doesn't re-investigate the same idea next wake.

Like the Worker, the Research agent gets no direct tracker credential — see [harness/tracker isolation](harness-tracker-isolation.md).
