//! Harness + auth profile list (docs/design.md, resolved decision #8).
//!
//! Dispatch is a prioritized list of `(harness, auth mode)` profiles, subscription
//! first, falling through to the next profile only on a *detected block* — a typed
//! error signal from the harness (rate limit / billing / auth), not any nonzero
//! exit code.

use serde::{Deserialize, Serialize};
use std::process::ExitStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthMode {
    /// Reads the harness's existing subscription login (e.g. plain `claude -p`,
    /// `codex exec` with no API key env var set).
    Subscription,
    /// Forces metered API-key billing (e.g. `claude --bare -p` + `ANTHROPIC_API_KEY`,
    /// or `codex exec` with `OPENAI_API_KEY` set).
    ApiKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessProfile {
    pub name: String,
    /// Command template — the binary and static args; the actual prompt/task is
    /// appended at dispatch time.
    pub cmd: String,
    pub args: Vec<String>,
    pub auth_mode: AuthMode,
    /// Lower runs first. Profiles for the same logical harness typically pair a
    /// subscription profile (priority 1) with an API-key fallback (priority 2).
    pub priority: u8,
}

/// Why a dispatch attempt was considered blocked and should fall through to the
/// next profile, rather than being treated as a task failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    RateLimit,
    BillingError,
    AuthError,
}

pub struct DispatchOutcome {
    pub status: ExitStatus,
    pub stdout: String,
    pub stderr: String,
}

/// Runs profiles in priority order until one succeeds or none are left.
///
/// Real implementation spawns each profile's command via `tokio::process::Command`,
/// inspects the output stream for the harness's typed block signals (Claude Code's
/// `system/api_retry` event `error` field; Codex's analogous surface), and only
/// then decides whether to fall through — not on exit-code alone.
pub async fn dispatch_with_fallback(
    profiles: &[HarnessProfile],
    prompt: &str,
) -> anyhow::Result<DispatchOutcome> {
    let _ = (profiles, prompt);
    todo!("try each profile in priority order, detect BlockReason, fall through")
}

/// Inspects a harness's output for a known block signal. Stubbed per-harness parsing.
pub fn detect_block(_stdout: &str, _stderr: &str) -> Option<BlockReason> {
    todo!("parse harness-specific typed error events")
}
