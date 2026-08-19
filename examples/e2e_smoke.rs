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
use lucid::harness::{AuthMode, ExecutionBackend, HarnessKind, HarnessProfile};
use lucid::tracker::file::FileTracker;
use lucid::tracker::{DecisionState, EffortEstimate, Proposal, ReviewMode, TrackerAdapter};
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
        execution_backend: ExecutionBackend::Sandboxed,
        unsandboxed: false,
    }];

    let observability = ObservabilityConfig {
        otlp_endpoint: "http://localhost:4317".to_string(),
        log_prompts: false,
        trace_ui_base_url: "http://localhost:6006".to_string(),
        trace_ui_project_id: None,
    };

    // A real git repo, checked out on `main` — every dispatch's worktree
    // branches off this tip (see `worktree::create`). No `origin` remote here, so
    // `gh pr create` will fail; `dispatch_and_finalize` degrades gracefully in
    // that case (attaches a note, skips the merge, still lands on `Done`) rather
    // than failing the whole dispatch — this smoke test exercises that path too.
    let run_git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(&workdir)
            .output()
    };
    run_git(&["init", "-q", "-b", "main"])?;
    run_git(&["config", "user.email", "smoke@example.com"])?;
    run_git(&["config", "user.name", "smoke"])?;
    std::fs::write(workdir.join("README.md"), "smoke\n")?;
    run_git(&["add", "-A"])?;
    run_git(&["commit", "-q", "-m", "init"])?;

    let worktree_root = std::env::temp_dir().join("lucid-e2e-smoke-worktrees");

    // Files a full proposal (title + frontmatter/body handoff surface) and
    // approves it — exercises the same create_proposal -> set_decision_state ->
    // query_by_label("proposal:approved") path `daemon::dispatch_approved_issues`
    // uses, instead of hand-building a bare TrackerIssue.
    let proposal = Proposal {
        title: "Add a NOTES.md file".to_string(),
        summary: "Create NOTES.md with the single line 'smoke test'.".to_string(),
        why_now: vec!["e2e smoke coverage".to_string()],
        effort_estimate: EffortEstimate::Small,
        risk_note: "none".to_string(),
        task_type: "chore".to_string(),
        target_paths: vec!["NOTES.md".to_string()],
        acceptance_criteria: vec!["NOTES.md exists and contains 'smoke test'".to_string()],
        research_ref: None,
        review: ReviewMode::Auto,
        verify_cmd: None,
    };
    let issue_id = tracker.create_proposal(&proposal).await?;
    tracker
        .set_decision_state(&issue_id, DecisionState::Approved)
        .await?;

    let approved = tracker
        .query_by_decision_state(DecisionState::Approved)
        .await?;
    let issue = approved
        .into_iter()
        .find(|i| i.id == issue_id)
        .expect("just-approved issue should be visible to query_by_label");

    let comments = tracker.list_comments(&issue.id).await?;
    println!(
        "--- dispatch prompt ---\n{}\n",
        worker::dispatch_prompt(&issue, &comments)
    );

    println!(
        "dispatching against {} (worktree under {})",
        workdir.display(),
        worktree_root.display()
    );
    let run = worker::dispatch_and_finalize(
        &tracker,
        &issue,
        &profiles,
        &observability,
        &workdir,
        &worktree_root,
        "main",
        std::time::Duration::from_secs(120),
        None,
    )
    .await?;

    println!("\n--- WorkerRun ---");
    println!("phase:       {:?}", run.phase);
    println!("dispatch_id: {:?}", run.dispatch_id);
    println!("session_id:  {:?}", run.session_id);
    println!("last_error:  {:?}", run.last_error);

    let final_state = tracker
        .query_similar(&issue.title)
        .await?
        .into_iter()
        .find(|i| i.id == issue.id)
        .and_then(|i| i.decision_state);
    println!("\nfinal decision_state: {final_state:?}");

    println!(
        "\n(full record, including the posted notes/trace link, is in {})",
        tracker_path.display()
    );

    Ok(())
}
