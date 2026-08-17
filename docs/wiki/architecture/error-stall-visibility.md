# Error/Stall Visibility — the Most Consistent Gap Across Every System

## cyrus's issue tracker is the strongest evidence found in either research pass

More valuable than any marketing page — a live, open-source issue tracker documenting real, unresolved operational failures:

- Silent Linear API rate-limit stalls that break in-flight sessions with no surfacing (#1324).
- A completed session's result silently discarded after an interrupt (#1389).
- An infinite retry loop burning API quota with no visible signal (#1378).
- Runaway self-replicating sessions (#1328).
- Unbounded memory growth from uncleaned completed sessions, leading to OOM (#1381).
- A cloud job that stayed visible but never executed, with zero progress comment (#1250).

This confirms, in a live tracker rather than an anecdote, exactly the "silent/latent failure is the dominant failure mode" finding from [practitioner reality](../research/practitioner-reality.md).

## What lucid's design doesn't yet address

- Rate-limit-specific handling as a distinct failure class (vs. generic retry).
- A runaway/self-replicating-session guard.
- Bounded cleanup of completed session state.
- Persistence of blocked/error state across a restart (Symphony's own dashboard state doesn't survive a restart either — worth not inheriting that specific weakness; see [tech stack](tech-stack.md) for why `rusqlite` was chosen partly for this reason).

## The plausible differentiator

No system surveyed has solved *proactive* stall notification — every one relies on the human noticing or polling a dashboard/log, not on an active push when something goes quiet. Given lucid is already building on Linear (mobile push) specifically to solve the remote-visibility requirement (see [observability](observability.md)), closing this gap — an active "Worker stalled, here's why" push rather than passive dashboard-checking — is a plausible place to actually do better than every system surveyed, not just match them.

Source: `docs/design.md` § UX / State-Machine Gap Analysis → Error/stall visibility.
