//! Notification sink (see docs/wiki/architecture/human-in-the-loop.md and
//! docs/wiki/architecture/extensibility-primitives.md). Fires when a human needs
//! to look at something — a pluggable, config-selected trait, same shape as
//! `TrackerAdapter`. Never allowed to be fatal to a caller: every call site logs
//! and continues on error rather than propagating with `?`.

pub mod null;
pub mod script;

use crate::tracker::TrackerIssue;

#[async_trait::async_trait]
pub trait NotificationSink: Send + Sync {
    /// Not yet called anywhere — see docs/wiki/architecture/human-in-the-loop.md's
    /// `NEEDS_INPUT:` marker parsing, a separate, lower-priority piece. Exists on
    /// the trait now so every implementation carries the full contract from day
    /// one.
    async fn on_awaiting_input(&self, issue: &TrackerIssue, question: &str) -> anyhow::Result<()>;
    async fn on_needs_review(
        &self,
        issue: &TrackerIssue,
        pr_url: Option<&str>,
    ) -> anyhow::Result<()>;
    async fn on_done(&self, issue: &TrackerIssue) -> anyhow::Result<()>;
}

/// Builds the configured `NotificationSink` — the one place `notifications`
/// config gets interpreted. `backend` (mirroring `TrackerConfig::backend`)
/// selects `"null"` (default) or `"script"`.
///
/// `[notifications]` is a single global config section; in multi-project mode
/// every project shares one `script_dir`, resolved against `workdir` (today's
/// `daemon.workdir`) rather than each project's own path — same global scoping
/// `observability` already has. Documented deferral, not a silent gap.
///
/// # Errors
/// Returns an error for an unrecognized `backend`.
pub fn build(
    config: &crate::config::NotificationConfig,
    workdir: &std::path::Path,
) -> anyhow::Result<Box<dyn NotificationSink>> {
    match config.backend.as_str() {
        "null" => Ok(Box::new(null::NullSink)),
        "script" => {
            let dir = workdir.join(&config.script_dir);
            Ok(Box::new(script::ScriptSink::new(
                dir,
                std::time::Duration::from_secs(config.timeout_secs),
            )))
        }
        other => Err(anyhow::anyhow!(
            "unknown notifications.backend `{other}` (expected \"null\" or \"script\")"
        )),
    }
}
