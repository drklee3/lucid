# UX Principles for the CLI and Extension Contract

Source: [lawsofux.com](https://lawsofux.com/), fetched directly 2026-08-19 (30 laws, current list — not from memory, since a curated list like this can and does change). lucid has two distinct UX surfaces, both text/protocol, neither graphical:

1. **Operator-facing** — `lucid status`/`show`/`task approve`/`presence`, error messages, and notifications (`NotificationSink`/`ScriptSink`, see [human-in-the-loop](human-in-the-loop.md)).
2. **Extension-author-facing** — the script/plugin contract from [extensibility-primitives](extensibility-primitives.md): discovery convention, manifest files, JSON-RPC responses, timeouts/backoff, JSON-Schema-as-documentation.

Most of lawsofux.com's laws were written with graphical/visual interfaces in mind. Rather than force-fitting all 30 as equally applicable, each is judged honestly below: concretely applicable (with a specific lucid tie-in), adapted/secondary (the underlying idea transfers from visual to textual, but weakly), or not relevant (and why).

## Concretely applicable

**Chunking** — "Individual pieces of an information set are broken down and then grouped together in a meaningful whole." Applies directly to `docs/CLI.md`'s command tree, which is already chunked by namespace (`task`, `presence`, `config`) rather than a flat list of 20 verbs — and to `lucid status` output, which should group by project/decision-state rather than one undifferentiated list once multi-project output actually needs it.

**Flow** — "Fully immersed in a feeling of energized focus, full involvement, and enjoyment." The justification for keeping `NotificationSink` to exactly three events (`on_awaiting_input`/`on_needs_review`/`on_done`) instead of firing on every `WorkerPhase` transition — an operator's own flow state is the thing lucid is explicitly designed not to interrupt unnecessarily while autonomous dispatch runs in the background; notifications exist only for the moments that actually need attention.

**Jakob's Law** — "Users spend most of their time on other sites/products... they prefer yours to work the same way as the ones they already know." The single most load-bearing law here, and it retroactively validates most of what [extensibility-primitives](extensibility-primitives.md) already decided: reusing git-hooks' discovery convention instead of inventing one, reusing real JSON-RPC 2.0 instead of a bespoke envelope, method names in the wire protocol matching the Rust trait method names 1:1. Every one of those choices exists specifically so a plugin author's existing knowledge transfers instead of requiring them to learn something lucid-specific for no reason.

**Mental Model** — "A compressed model based on what we think we know about a system and how it works." Already explicitly protected in one place: `docs/CLI.md` states `lucid status`'s `STATE` column "come[s] directly from the Worker phase enum... a rendering of that state machine, not a separate concept" — no translation layer for an operator to keep straight. The extension contract should hold the same line: JSON-RPC `method` names are the literal trait method names, not a prettified API surface layered on top that a plugin author would have to learn as a second vocabulary.

**Postel's Law** — "Be liberal in what you accept, and conservative in what you send." Directly actionable, and not yet stated anywhere in [extensibility-primitives](extensibility-primitives.md)'s schema/versioning design — added there now: the daemon deserializing a plugin's JSON-RPC response should tolerate unknown/extra fields rather than hard-failing (`serde`'s default behavior, i.e. explicitly *not* `#[serde(deny_unknown_fields)]`), matching the semver MINOR-additive compatibility rule already chosen — a plugin author adding a field the daemon doesn't understand yet shouldn't break. In the other direction, everything lucid itself sends to a plugin should be strictly schema-valid, compact JSON, no ambiguity — the daemon is the stricter party since it's the one party guaranteed to be running the current version.

**Tesler's Law** (Law of Conservation of Complexity) — "For any system there is a certain amount of complexity which cannot be reduced [only moved]." The right frame for *why* lucid should own timeout/backoff/schema-validation machinery centrally rather than "just spawn a process and hope": that complexity doesn't disappear if lucid skips designing it — it just gets redundantly reinvented, worse, by every plugin author individually. The restart/backoff and timeout defaults already resolved in [extensibility-primitives](extensibility-primitives.md) are complexity lucid is deliberately absorbing once instead of pushing onto N plugin authors.

**Hick's Law** / **Choice Overload** — "Decision time increases with the number and complexity of choices" / "the tendency to get overwhelmed by a large number of options." Both reinforce a decision already made rather than introducing a new one: [extensibility-primitives](extensibility-primitives.md)'s hook surface is deliberately 2–3 named points, not a hook per pipeline stage, and `docs/CLI.md`'s command tree stays a small, flat, namespaced set (`task`, `presence`, `config`) rather than growing per-feature flags. Worth keeping explicit as a constraint on the still-open `DispatchPolicy` question: whatever shape it takes, it shouldn't grow the decision surface an operator has to hold in their head.

**Occam's Razor** — "Among competing hypotheses that predict equally well, the one with the fewest assumptions should be selected." This is the extensibility page's own governing rule restated: default to a generic script-backed implementation; only add bespoke Rust (a new assumption baked into the binary) when hot-path, security, or typed-correctness actually demands it.

**Doherty Threshold** — "Productivity soars when a computer and its users interact at a pace (<400ms) that ensures neither has to wait on the other." Applies specifically to *synchronous* operator commands — `lucid status`, `lucid task approve` — not to the async background dispatch loop, which legitimately takes minutes. Concrete constraint: these commands should read from local state (`DaemonState`, tracker cache) rather than doing a slow synchronous round-trip (e.g. a live Linear API call) inline, or they'll blow well past 400ms and the tool will feel laggy exactly where it's most interactive.

**Parkinson's Law** — "Any task will inflate until all of the available time is spent." The actual justification for every timeout that exists or was just added: `stall_timeout_secs` (harness dispatch), and the newly-chosen 5s handshake / 10s per-call timeouts for script plugins. An unbounded subprocess call doesn't fail fast, it just runs — the timeout is what turns "inflate forever" into "fail loud, retry with backoff."

**Zeigarnik Effect** — "People remember uncompleted or interrupted tasks better than completed tasks." The psychological mechanism `NotificationSink`'s three events actually lean on: `on_awaiting_input`/`on_needs_review` exist to surface the *incomplete* thing, then let the operator's own attention do the rest — the notification doesn't need to be naggy or repeated, because an unresolved item already has a cognitive pull `on_done` doesn't need at all (nobody needs reminding about something finished).

**Peak-End Rule** — "People judge an experience largely based on how they felt at its peak and at its end." Validates where [worker-completion](worker-completion.md) already concentrates its design effort: the *end* of a dispatch (PR body, commit summary, the note attached on failure) matters disproportionately more than getting every intermediate `WorkerPhase` transition perfectly polished — and that's already where the detail lives.

**Cognitive Load** / **Miller's Law** / **Working Memory** — closely related (Miller's "7±2 items," Cognitive Load's "mental resources to understand an interface," Working Memory's "temporarily holds info for tasks"), grouped because they cash out the same way here: keep any one table/output/config section within roughly that range before it needs sub-grouping. Worth a real check, not just a platitude — `[daemon]`'s config table (`docs/CLI.md`'s config section) is at 7 fields, right at the edge; `lucid status`'s example table has 6 columns. Both currently fine, but it's the concrete ceiling to notice crossing later, not just an abstract nicety.

## Adapted / secondary

These are real Gestalt/visual-grouping laws (**Law of Common Region**, **Law of Proximity**, **Law of Prägnanz**, **Law of Similarity**, **Law of Uniform Connectedness**) written for graphical layout. They transfer to a text CLI only loosely — grouping related fields on one line or under one heading in `lucid status`/`show` output (Proximity/Common Region), preferring the plainest output format that still conveys the state (Prägnanz), consistent styling for same-type values like all `DecisionState`s (Similarity). Real, but secondary to the concretely-applicable set above — worth a glance when actually designing `lucid status`'s exact table/column layout, not a driving constraint on the extension contract.

**Aesthetic-Usability Effect** ("aesthetically pleasing perceived as more usable") and **Von Restorff Effect** ("the different one is remembered") — both real but weakly text-CLI-applicable: consistent, clean table formatting for the former; color/highlighting a `Failed`/`Blocked` row differently from `Running` rows for the latter (`lucid status`'s color scheme, not yet designed). Minor, worth applying when the actual terminal output gets built, not architecturally significant now.

**Goal-Gradient Effect** ("approach to a goal increases with proximity"), **Selective Attention** ("focus on a subset of stimuli related to goals"), **Serial Position Effect** ("best remember first and last in a series") — plausible influences on `lucid status`'s default filtering (show active/actionable items, not the full history — already true in the CLI doc's example) and ordering (most urgent first), but speculative until that output is actually designed in detail.

**Pareto Principle** ("80% of effects from 20% of causes") — a scoping heuristic more than a UX law here: useful for deciding what to build first (a `NotificationSink` pilot covering one real use case, not every notification platform), already the operating mode of this whole design thread rather than a new instruction.

## Not relevant

**Fitts's Law** ("time to acquire a target is a function of distance to and size of the target") — a pointer/touch-targeting law. lucid has no pointer-based interaction surface at all (terminal CLI, JSON-RPC over stdio); genuinely inapplicable, not just low-priority.

**Cognitive Bias** ("a systematic error of thinking... that influences perception and decision-making") — a general cognitive-science caveat about human judgment, not an actionable interface-design law the way the other 29 are. No specific lucid tie-in beyond "be aware humans are biased," which doesn't inform any concrete decision here.

**Paradox of the Active User** ("users never read manuals, start using the software immediately") — real, but the tie-in is thin for a daemon an operator configures once via a TOML file and rarely touches interactively afterward; more applicable to the extension contract (a plugin author will absolutely start writing a script before reading `extensibility-primitives.md` end to end), which is already covered under Postel's Law above (self-explanatory error messages naming valid methods/schema, not requiring the doc to be read first) rather than needing its own separate section.

## Action taken from this pass

Postel's Law surfaced a real, previously-unstated gap: added a "liberal parsing" rule to [extensibility-primitives](extensibility-primitives.md) § Schema and versioning — the daemon deserializes plugin responses tolerating unknown fields, consistent with the MINOR-additive compatibility rule already decided there.

## Reference: all 30 laws, verbatim

Kept here so future design work can check a law's actual definition without re-fetching the site — general-purpose reference, not lucid-specific judgment (that's the sections above). Order as listed on [lawsofux.com](https://lawsofux.com/) as of 2026-08-19.

1. **Aesthetic-Usability Effect** — "Users often perceive aesthetically pleasing design as design that's more usable."
2. **Choice Overload** — "The tendency for people to get overwhelmed when they are presented with a large number of options."
3. **Chunking** — "A process by which individual pieces of an information set are broken down and then grouped together in a meaningful whole."
4. **Cognitive Bias** — "A systematic error of thinking or rationality in judgment that influence our perception of the world and our decision-making ability."
5. **Cognitive Load** — "The amount of mental resources needed to understand and interact with an interface."
6. **Doherty Threshold** — "Productivity soars when a computer and its users interact at a pace (<400ms) that ensures that neither has to wait on the other."
7. **Fitts's Law** — "The time to acquire a target is a function of the distance to and size of the target."
8. **Flow** — "The mental state in which a person performing some activity is fully immersed in a feeling of energized focus, full involvement, and enjoyment."
9. **Goal-Gradient Effect** — "The tendency to approach a goal increases with proximity to the goal."
10. **Hick's Law** — "The time it takes to make a decision increases with the number and complexity of choices."
11. **Jakob's Law** — "Users spend most of their time on other sites. This means that users prefer your site to work the same way as all the other sites they already know."
12. **Law of Common Region** — "Elements tend to be perceived into groups if they are sharing an area with a clearly defined boundary."
13. **Law of Proximity** — "Objects that are near, or proximate to each other, tend to be grouped together."
14. **Law of Prägnanz** — "People will perceive and interpret ambiguous or complex images as the simplest form possible, because it requires the least cognitive effort."
15. **Law of Similarity** — "The human eye tends to perceive similar elements as a complete picture, shape, or group, even if those elements are separated."
16. **Law of Uniform Connectedness** — "Elements that are visually connected are perceived as more related than elements with no connection."
17. **Mental Model** — "A compressed model based on what we think we know about a system and how it works."
18. **Miller's Law** — "The average person can only keep 7 (plus or minus 2) items in their working memory."
19. **Occam's Razor** — "Among competing hypotheses that predict equally well, the one with the fewest assumptions should be selected."
20. **Paradox of the Active User** — "Users never read manuals but start using the software immediately."
21. **Pareto Principle** — "For many events, roughly 80% of the effects come from 20% of the causes."
22. **Parkinson's Law** — "Any task will inflate until all of the available time is spent."
23. **Peak-End Rule** — "People judge an experience largely based on how they felt at its peak and at its end, rather than the total sum or average of every moment."
24. **Postel's Law** — "Be liberal in what you accept, and conservative in what you send."
25. **Selective Attention** — "The process of focusing our attention only to a subset of stimuli in an environment — usually those related to our goals."
26. **Serial Position Effect** — "Users have a propensity to best remember the first and last items in a series."
27. **Tesler's Law** — "For any system there is a certain amount of complexity which cannot be reduced."
28. **Von Restorff Effect** — "When multiple similar objects are present, the one that differs from the rest is most likely to be remembered."
29. **Working Memory** — "A cognitive system that temporarily holds and manipulates information needed to complete tasks."
30. **Zeigarnik Effect** — "People remember uncompleted or interrupted tasks better than completed tasks."

Applying one of these to a *new* lucid decision later — re-check it against the sections above first; if it's not judged there yet, add it rather than re-deriving from scratch each time.

## Related pages

- [extensibility-primitives](extensibility-primitives.md) — the extension-author surface this page evaluates
- [human-in-the-loop](human-in-the-loop.md) — `NotificationSink`
- [worker-completion](worker-completion.md) — where Peak-End Rule's tie-in already shows up in practice
- `docs/CLI.md` — the operator-facing command surface
