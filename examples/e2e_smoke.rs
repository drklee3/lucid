//! Manual, opt-in end-to-end smoke test: dispatches one real `claude -p` run
//! (subscription auth) and prints the resulting `WorkerRun` plus the trace link
//! that got posted to a local file-backed tracker. Not part of `cargo test` —
//! this hits the real `claude` binary and (if reachable) a real OTLP collector,
//! neither of which belong in the normal test suite.
//!
//! Run: `cargo run --example e2e_smoke`
//! Requires: `claude` on PATH, already logged in via `claude auth login`
//! (subscription). A Phoenix container at localhost:4317/6006 is optional — if
//! it's not running, the OTLP export just fails silently and the dispatch still
//! completes; see docker-compose.yml.

use lucid::config::ObservabilityConfig;
use lucid::harness::{AuthMode, HarnessKind, HarnessProfile};
use lucid::tracker::file::FileTracker;
use lucid::tracker::TrackerAdapter;
use lucid::worker;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let workdir = std::env::temp_dir().join("lucid-e2e-smoke");
    std::fs::create_dir_all(&workdir)?;

    let tracker_path = workdir.join("tracker.json");
    let tracker = FileTracker::open(&tracker_path)?;

    let profiles = [HarnessProfile {
        name: "claude-subscription".to_string(),
        kind: HarnessKind::ClaudeCode,
        cmd: "claude".to_string(),
        args: vec!["-p".to_string()],
        auth_mode: AuthMode::Subscription,
        priority: 1,
    }];

    let observability = ObservabilityConfig {
        otlp_endpoint: "http://localhost:4317".to_string(),
        log_prompts: false,
        trace_ui_base_url: "http://localhost:6006".to_string(),
        trace_ui_project_id: None,
    };

    let prompt =
        "Reply with exactly the word OK and don't read, write, or run anything else.";

    println!("dispatching against {}", workdir.display());
    let run = worker::run_dispatch(
        &tracker,
        "SMOKE-1",
        prompt,
        &profiles,
        &observability,
        &workdir,
        std::time::Duration::from_secs(120),
    )
    .await?;

    println!("\n--- WorkerRun ---");
    println!("phase:       {:?}", run.phase);
    println!("dispatch_id: {:?}", run.dispatch_id);
    println!("session_id:  {:?}", run.session_id);
    println!("last_error:  {:?}", run.last_error);

    println!("\n--- tracker notes for SMOKE-1 ---");
    for issue in tracker.query_similar("SMOKE").await? {
        println!("{} — {}", issue.id, issue.title);
    }
    println!(
        "\n(full record, including the posted note/trace link, is in {})",
        tracker_path.display()
    );

    Ok(())
}
