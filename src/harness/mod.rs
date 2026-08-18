//! Harness + auth profile list (see docs/wiki/architecture/harness-dispatch.md).
//!
//! Dispatch is a prioritized list of `(harness, auth mode)` profiles, subscription
//! first, falling through to the next profile only on a *detected block* — a typed
//! error signal from the harness (rate limit / billing / auth), not any nonzero
//! exit code. Block detection reads Claude Code's `--output-format stream-json`
//! event stream (`system/api_retry` events), not a text scan — see
//! `parse_stream_events` and docs/wiki/architecture/harness-dispatch.md.
//!
//! Every dispatch attempt is tagged with an `OTel` resource attribute pair
//! (`lucid.ticket_id`, `lucid.dispatch_id`) so the run's traces can be found again
//! from the tracker item later — see docs/wiki/architecture/trace-correlation.md.
//!
//! Every attempt also runs under a hard timeout (`kill_on_drop` + `tokio::time::
//! timeout`) — without one, a hung harness process hangs the whole daemon's
//! reconciliation tick forever, which is exactly the "silent stall" failure mode
//! the design survey flagged as the most common real-world failure (see
//! docs/wiki/architecture/error-stall-visibility.md).

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::ExitStatus;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthMode {
    /// Reads the harness's existing subscription login (e.g. plain `claude -p`,
    /// `codex exec` with no API key env var set).
    Subscription,
    /// Forces metered API-key billing (e.g. `claude --bare -p` + `ANTHROPIC_API_KEY`,
    /// or `codex exec` with `OPENAI_API_KEY` set).
    ApiKey,
}

/// Which coding harness a profile dispatches to. Drives telemetry injection and
/// dispatch-flag shape — Claude Code is env-var driven with a JSON event stream;
/// Codex reads `otel.*` from its own TOML config and has no equivalent permission
/// mode wired up yet (Codex's `approval_policy` isn't touched by this module).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HarnessKind {
    ClaudeCode,
    Codex,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarnessProfile {
    pub name: String,
    pub kind: HarnessKind,
    /// Command template — the binary and static args; the actual prompt/task is
    /// appended at dispatch time.
    pub cmd: String,
    pub args: Vec<String>,
    pub auth_mode: AuthMode,
    /// Lower runs first. Profiles for the same logical harness typically pair a
    /// subscription profile (priority 1) with an API-key fallback (priority 2).
    pub priority: u8,
}

/// Where to send OTLP traces/logs for dispatched harness runs, and whether to opt
/// into prompt/tool content capture (off by default on both harnesses).
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    pub otlp_endpoint: String,
    pub log_prompts: bool,
}

/// Why a dispatch attempt was considered blocked and should fall through to the
/// next profile, rather than being treated as a task failure. Maps from the
/// `system/api_retry` event's `error` field — only the categories another profile
/// could plausibly route around; `overloaded`/`server_error`/`unknown`/etc. are
/// real task failures, not fallback triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockReason {
    RateLimit,
    BillingError,
    AuthError,
}

impl BlockReason {
    fn from_retry_error(error: &str) -> Option<Self> {
        match error {
            "rate_limit" => Some(BlockReason::RateLimit),
            "billing_error" => Some(BlockReason::BillingError),
            "oauth_org_not_allowed" | "authentication_failed" => Some(BlockReason::AuthError),
            _ => None,
        }
    }
}

/// Distinguishes *why* a dispatch didn't return a normal outcome — a stalled
/// process (kill + retry, worth a `WorkerPhase::TimedOut`) is a different caller
/// response than every profile being blocked (a real, if unusual, task failure).
#[derive(Debug, thiserror::Error)]
pub enum DispatchError {
    #[error("dispatch to `{profile}` timed out after {timeout:?}")]
    Timeout { profile: String, timeout: Duration },
    #[error("all harness profiles blocked; last: {profile} ({reason:?})")]
    AllBlocked {
        profile: String,
        reason: BlockReason,
    },
    #[error("no harness profiles configured")]
    NoProfiles,
}

#[derive(Debug, Clone, Default)]
pub struct DispatchOutcome {
    /// Correlation id for this dispatch attempt, also embedded in the harness's own
    /// `OTel` traces via `lucid.dispatch_id` — this is what a tracker comment links to.
    pub dispatch_id: String,
    pub profile_name: String,
    pub status: Option<ExitStatus>,
    /// Claude Code's own session id, from the `system/init` or `result` event —
    /// `None` if the harness never got far enough to emit one (see
    /// docs/wiki/architecture/agent-handoff.md for how a Worker would use this to
    /// resume).
    pub session_id: Option<String>,
    /// The `result` event's `is_error` field, when a result event was seen.
    pub is_error: Option<bool>,
    /// The `result` event's `result` text field, when present.
    pub result_text: Option<String>,
    pub stdout: String,
    pub stderr: String,
}

/// One event from `--output-format stream-json`. Only the fields lucid currently
/// reads are modeled — the full message schema isn't exhaustively documented
/// upstream (see `anthropics/claude-code#24612`), so this is intentionally partial
/// and permissive (`#[serde(default)]` everywhere) rather than a strict schema that
/// breaks on an unrecognized event shape.
#[derive(Debug, Deserialize)]
struct StreamEvent {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    subtype: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    is_error: Option<bool>,
    #[serde(default)]
    result: Option<String>,
}

/// Parses newline-delimited stream-json output, folding it into the block-reason
/// (from the *last* `system/api_retry` event, if any — an earlier retry that then
/// succeeded shouldn't count) and the final result metadata (from the last `result`
/// event). Malformed/unrecognized lines are skipped rather than failing the whole
/// parse — stderr or a stray non-JSON line shouldn't take down block detection.
fn parse_stream_events(stdout: &str) -> (Option<BlockReason>, DispatchOutcome) {
    let mut block = None;
    let mut outcome = DispatchOutcome::default();

    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<StreamEvent>(line) else {
            continue;
        };
        match event.kind.as_str() {
            "system" if event.subtype.as_deref() == Some("api_retry") => {
                block = event
                    .error
                    .as_deref()
                    .and_then(BlockReason::from_retry_error);
            }
            "system" if event.subtype.as_deref() == Some("init") => {
                outcome.session_id = event.session_id.or(outcome.session_id);
            }
            "result" => {
                outcome.session_id = event.session_id.or(outcome.session_id);
                outcome.is_error = event.is_error;
                outcome.result_text = event.result;
            }
            _ => {}
        }
    }

    (block, outcome)
}

/// Tags the subprocess with the standard `OTEL_RESOURCE_ATTRIBUTES` env var (a
/// generic `OTel` SDK var, not harness-specific) plus each harness's own telemetry
/// on-switch. Codex ignores `OTEL_RESOURCE_ATTRIBUTES` (its `OTel` config is
/// TOML-driven, user-level only) so it gets the OTLP endpoint via `-c` instead;
/// per-dispatch resource tagging for Codex isn't wired up yet.
fn apply_telemetry(
    cmd: &mut tokio::process::Command,
    kind: HarnessKind,
    telemetry: &TelemetryConfig,
    ticket_id: &str,
    dispatch_id: &str,
) {
    match kind {
        HarnessKind::ClaudeCode => {
            cmd.env("CLAUDE_CODE_ENABLE_TELEMETRY", "1")
                // Distributed tracing (spans) is a separate, off-by-default beta
                // signal from logs/metrics — `CLAUDE_CODE_ENABLE_TELEMETRY` alone
                // does not turn it on. Without both of these, `trace_link`'s URL
                // points at spans that were never exported. Source:
                // https://code.claude.com/docs/en/monitoring-usage
                .env("CLAUDE_CODE_ENHANCED_TELEMETRY_BETA", "1")
                .env("OTEL_TRACES_EXPORTER", "otlp")
                .env("OTEL_LOGS_EXPORTER", "otlp")
                .env("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc")
                .env("OTEL_EXPORTER_OTLP_ENDPOINT", &telemetry.otlp_endpoint)
                .env(
                    "OTEL_RESOURCE_ATTRIBUTES",
                    format!("lucid.ticket_id={ticket_id},lucid.dispatch_id={dispatch_id}"),
                );
            if telemetry.log_prompts {
                cmd.env("OTEL_LOG_USER_PROMPTS", "1")
                    .env("OTEL_LOG_TOOL_DETAILS", "1");
            }
        }
        HarnessKind::Codex => {
            cmd.arg("-c").arg(format!(
                "otel.exporter={{otlp-grpc={{endpoint=\"{}\"}}}}",
                telemetry.otlp_endpoint
            ));
            if telemetry.log_prompts {
                cmd.arg("-c").arg("otel.log_user_prompt=true");
            }
        }
    }
}

/// Adds the flags that make a Claude Code dispatch actually parseable unattended:
/// structured event output, so block detection and session-id capture work at all.
/// `extra_args` carries the caller-specific permission surface — a Worker's full
/// `--permission-mode auto` versus a read-only PM investigation's `--allowedTools`
/// list — since those two callers need genuinely different tool access, not a
/// single hardcoded mode. `claude -p` always starts in Manual mode regardless of
/// plan tier, so *some* explicit grant is required or the first tool call blocks
/// waiting for an approval nobody's there to give. See
/// docs/wiki/architecture/harness-dispatch.md.
fn apply_dispatch_flags(cmd: &mut tokio::process::Command, kind: HarnessKind, extra_args: &[&str]) {
    if kind == HarnessKind::ClaudeCode {
        cmd.args(["--output-format", "stream-json", "--verbose"]);
        cmd.args(extra_args);
    }
}

/// Everything one `dispatch_with_fallback` call needs — bundled into a struct
/// rather than seven positional args, and so a caller (Worker vs. PM) can vary
/// `claude_extra_args`/`timeout` without every call site changing shape.
pub struct DispatchRequest<'a> {
    pub profiles: &'a [HarnessProfile],
    pub prompt: &'a str,
    pub ticket_id: &'a str,
    pub telemetry: &'a TelemetryConfig,
    /// Directory the harness runs in — a git worktree once worktree management
    /// exists (docs/FEATURES.md § Worker / dispatch); any directory works for now.
    pub workdir: &'a Path,
    /// Hard per-attempt wall-clock limit. On expiry the child process is killed
    /// (`kill_on_drop`) and this returns `DispatchError::Timeout` rather than
    /// trying the next profile — a stall isn't an auth-type block signal, so it's
    /// not something switching harness profile would fix.
    pub timeout: Duration,
    /// Extra Claude-Code-specific CLI args appended after the telemetry/output
    /// flags — e.g. `&["--permission-mode", "auto"]` for a Worker's full-access
    /// dispatch, or `&["--allowedTools", "Read,Grep,Glob"]` for a read-only PM
    /// investigation. Ignored for non-Claude-Code profiles.
    pub claude_extra_args: &'a [&'a str],
}

/// Runs profiles in priority order until one succeeds or none are left.
///
/// # Errors
/// Returns [`DispatchError`] (wrapped in `anyhow::Error` — downcast with
/// `error.downcast_ref::<DispatchError>()` to distinguish the cases) if every
/// profile was blocked, no profiles were configured, or an attempt timed out.
/// Returns a plain I/O error if spawning the subprocess itself failed.
pub async fn dispatch_with_fallback(req: DispatchRequest<'_>) -> anyhow::Result<DispatchOutcome> {
    let dispatch_id = Uuid::new_v4().to_string();

    let mut ordered: Vec<&HarnessProfile> = req.profiles.iter().collect();
    ordered.sort_by_key(|p| p.priority);

    let mut last_block: Option<(String, BlockReason)> = None;

    for profile in ordered {
        let mut cmd = tokio::process::Command::new(&profile.cmd);
        cmd.current_dir(req.workdir);
        cmd.kill_on_drop(true);
        cmd.args(&profile.args);
        apply_telemetry(
            &mut cmd,
            profile.kind,
            req.telemetry,
            req.ticket_id,
            &dispatch_id,
        );
        apply_dispatch_flags(&mut cmd, profile.kind, req.claude_extra_args);
        cmd.arg(req.prompt);

        let output = match tokio::time::timeout(req.timeout, cmd.output()).await {
            Ok(result) => result?,
            Err(_elapsed) => {
                return Err(DispatchError::Timeout {
                    profile: profile.name.clone(),
                    timeout: req.timeout,
                }
                .into());
            }
        };
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        let (block, mut outcome) = parse_stream_events(&stdout);
        if let Some(reason) = block {
            last_block = Some((profile.name.clone(), reason));
        } else {
            outcome.dispatch_id = dispatch_id;
            outcome.profile_name.clone_from(&profile.name);
            outcome.status = Some(output.status);
            outcome.stdout = stdout;
            outcome.stderr = stderr;
            return Ok(outcome);
        }
    }

    match last_block {
        Some((profile, reason)) => Err(DispatchError::AllBlocked { profile, reason }.into()),
        None => Err(DispatchError::NoProfiles.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(json: &str) -> String {
        format!("{json}\n")
    }

    fn telemetry() -> TelemetryConfig {
        TelemetryConfig {
            otlp_endpoint: "http://localhost:4317".to_string(),
            log_prompts: false,
        }
    }

    #[test]
    fn parse_stream_events_finds_the_last_api_retry_error() {
        let stdout = line(r#"{"type":"system","subtype":"init","session_id":"s1"}"#)
            + &line(r#"{"type":"system","subtype":"api_retry","error":"overloaded"}"#)
            + &line(r#"{"type":"system","subtype":"api_retry","error":"rate_limit"}"#);
        let (block, outcome) = parse_stream_events(&stdout);
        assert_eq!(block, Some(BlockReason::RateLimit));
        assert_eq!(outcome.session_id.as_deref(), Some("s1"));
    }

    #[test]
    fn parse_stream_events_ignores_non_blocking_retry_categories() {
        let stdout = line(r#"{"type":"system","subtype":"api_retry","error":"overloaded"}"#);
        let (block, _) = parse_stream_events(&stdout);
        assert_eq!(block, None);
    }

    #[test]
    fn parse_stream_events_reads_the_final_result() {
        let stdout = line(r#"{"type":"system","subtype":"init","session_id":"s1"}"#)
            + &line(
                r#"{"type":"result","subtype":"success","is_error":false,"session_id":"s1","result":"done"}"#,
            );
        let (block, outcome) = parse_stream_events(&stdout);
        assert_eq!(block, None);
        assert_eq!(outcome.is_error, Some(false));
        assert_eq!(outcome.result_text.as_deref(), Some("done"));
    }

    #[test]
    fn parse_stream_events_skips_malformed_lines() {
        let stdout = "not json\n".to_string()
            + &line(r#"{"type":"result","subtype":"success","is_error":false}"#);
        let (block, outcome) = parse_stream_events(&stdout);
        assert_eq!(block, None);
        assert_eq!(outcome.is_error, Some(false));
    }

    #[test]
    fn profiles_run_in_priority_order() {
        let profiles = [
            HarnessProfile {
                name: "b".into(),
                kind: HarnessKind::ClaudeCode,
                cmd: "true".into(),
                args: vec![],
                auth_mode: AuthMode::Subscription,
                priority: 2,
            },
            HarnessProfile {
                name: "a".into(),
                kind: HarnessKind::ClaudeCode,
                cmd: "true".into(),
                args: vec![],
                auth_mode: AuthMode::Subscription,
                priority: 1,
            },
        ];
        let mut ordered: Vec<&HarnessProfile> = profiles.iter().collect();
        ordered.sort_by_key(|p| p.priority);
        assert_eq!(ordered[0].name, "a");
        assert_eq!(ordered[1].name, "b");
    }

    #[tokio::test]
    async fn a_hanging_process_times_out_and_is_killed() {
        // `sh -c 'sleep 30'` rather than plain `sleep 30`: the extra flags
        // `apply_dispatch_flags`/`apply_telemetry` append (Claude-Code-specific,
        // and harmless here since a real harness would understand them) would
        // otherwise be parsed as invalid arguments by `sleep` itself and make it
        // exit immediately instead of actually hanging. `sh -c` ignores trailing
        // positional args the script body doesn't reference.
        let profiles = [HarnessProfile {
            name: "hangs".into(),
            kind: HarnessKind::ClaudeCode,
            cmd: "sh".into(),
            args: vec!["-c".into(), "sleep 30".into()],
            auth_mode: AuthMode::Subscription,
            priority: 1,
        }];
        let telemetry = telemetry();
        let req = DispatchRequest {
            profiles: &profiles,
            prompt: "irrelevant",
            ticket_id: "T-1",
            telemetry: &telemetry,
            workdir: &std::env::temp_dir(),
            timeout: Duration::from_millis(50),
            claude_extra_args: &["--permission-mode", "auto"],
        };
        let err = dispatch_with_fallback(req).await.unwrap_err();
        assert!(matches!(
            err.downcast_ref::<DispatchError>(),
            Some(DispatchError::Timeout { .. })
        ));
    }
}
