# lucid

Autonomous, presence-aware development orchestration. Named for lucid dreaming —
acts on its own, but stays directed within bounds you set; never fully unsupervised.

A standalone Rust daemon that, while you're away (presence-gated, not on a naive
schedule), investigates a repo against a stated goal, flags concrete gaps as
proposals for you to approve, and dispatches approved work to a coding harness
(`claude -p`, `codex exec`, or others) — reconciling state, retrying, and
reporting back the same way it would if you were watching.

Tracker-agnostic, harness-agnostic, model-agnostic by design. See
[`docs/wiki/index.md`](docs/wiki/index.md) for the full architecture, resolved
decisions, and grounding research; [`docs/FEATURES.md`](docs/FEATURES.md) for
exactly what's built vs. still open.

## Status

Working MVP, live-tested against real `claude -p` subscription dispatch — not a
prototype, but not feature-complete either:

- ✅ Presence-gated reconciliation loop (`lucid start`), PM gap-detection wake,
  Claude Code dispatch with block/timeout handling, file-backed and real Linear
  tracker adapters, OTel trace correlation back to the tracker item.
- ⛔ No per-issue git worktree isolation yet (dispatch runs in one configured
  directory), no cross-process `status`/`stop` (needs an IPC layer, not designed
  yet), state is in-memory only (no restart persistence).

See [`docs/FEATURES.md`](docs/FEATURES.md) for the itemized breakdown.

## Quickstart

```bash
cargo build --release

# minimal config — see docs/CLI.md for every field
cat > lucid.toml <<'EOF'
[[harness_profiles]]
name = "claude-subscription"
kind = "ClaudeCode"
cmd = "claude"
args = ["-p"]
auth_mode = "Subscription"
priority = 1

[tracker]
backend = "file"
file_path = "state/tracker.json"

[presence]
idle_threshold_minutes = 20
proposal_cap_per_wake = 3

[observability]
otlp_endpoint = "http://localhost:4317"
trace_ui_base_url = "http://localhost:6006"
EOF

docker compose up -d          # optional: Arize Phoenix, for trace correlation
./target/release/lucid config validate
./target/release/lucid presence override autonomous   # logind auto-detection isn't wired up yet
./target/release/lucid start --foreground
```

Full command reference: [`docs/CLI.md`](docs/CLI.md).
