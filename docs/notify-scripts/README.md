# Notification scripts

Copy-paste starting points for `[notifications].backend = "script"` (see [`docs/CONFIGURATION.md`](../CONFIGURATION.md) § `[notifications]`). Each file here is named for the event it handles — `on_needs_review`, `on_done` — matching the filename lucid looks for under `script_dir` (default `.lucid/notify/`).

To use one:

```bash
mkdir -p .lucid/notify
cp docs/notify-scripts/discord-on_needs_review .lucid/notify/on_needs_review
chmod +x .lucid/notify/on_needs_review
```

Every script receives one JSON object on stdin and nothing else — no arguments, no environment lucid sets for you beyond your own shell's. Shape (see [extensibility primitives](../wiki/architecture/extensibility-primitives.md)):

```json
{
  "protocol": "lucid.plugin/1",
  "event": "on_needs_review",
  "issue": {"id": "...", "title": "...", "identifier": "SUSHI-72", "review": "Auto", "decision_state": "Approved"},
  "pr_url": "https://github.com/org/repo/pull/42"
}
```

`pr_url` and `question` are omitted entirely (not `null`) when not applicable to that event. Exit code and stdout are ignored — lucid fires the script and moves on; a nonzero exit or a 10-second timeout is logged, never blocks or fails the dispatch itself.

## Files

- `discord-on_needs_review` — posts to a Discord webhook when a dispatch needs human review.
