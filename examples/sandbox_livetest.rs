//! Manual, opt-in live check that `ExecutionBackend::Sandboxed` actually
//! isolates the dispatched process from the host filesystem. Not part of
//! `cargo test` — requires Docker and `lucid-sandbox:latest` already built
//! (`docker build -t lucid-sandbox:latest -f docker/sandbox/Dockerfile .`).
//!
//! Run: `cargo run --example sandbox_livetest -- <worktree-dir> <host-only-file>`

use lucid::harness::{
    AuthMode, DispatchRequest, ExecutionBackend, HarnessKind, HarnessProfile, TelemetryConfig,
};
use std::path::PathBuf;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let workdir = PathBuf::from(&args[1]);
    let host_only_file = &args[2];
    let git_commit_probe = args.get(3).is_some_and(|s| s == "git-commit");

    let script = if git_commit_probe {
        "git -c user.email=a@a.com -c user.name=a commit -am 'sandboxed dispatch commit' 2>&1"
            .to_string()
    } else {
        format!(
            "echo INSIDE_WORKTREE:$(cat inside.txt 2>&1); \
             echo HOST_FILE_READ:$(cat {host_only_file} 2>&1); \
             echo write-from-container > wrote-from-container.txt; \
             echo WRITE_OK:$?"
        )
    };

    let profiles = [HarnessProfile {
        name: "sandbox-probe".to_string(),
        kind: HarnessKind::ClaudeCode,
        cmd: "sh".to_string(),
        args: vec!["-c".to_string(), script],
        auth_mode: AuthMode::Subscription,
        priority: 1,
        execution_backend: ExecutionBackend::Sandboxed,
        unsandboxed: false,
    }];

    let telemetry = TelemetryConfig {
        otlp_endpoint: "http://localhost:4317".to_string(),
        log_prompts: false,
    };

    let outcome = lucid::harness::dispatch_with_fallback(DispatchRequest {
        profiles: &profiles,
        prompt: "irrelevant",
        ticket_id: "LIVETEST-1",
        telemetry: &telemetry,
        workdir: &workdir,
        timeout: Duration::from_secs(30),
        claude_extra_args: &["--permission-mode", "auto"],
    })
    .await?;

    println!("--- stdout ---\n{}", outcome.stdout);
    println!("--- stderr ---\n{}", outcome.stderr);
    println!("--- status ---\n{:?}", outcome.status);

    Ok(())
}
