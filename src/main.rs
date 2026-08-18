// Every `Command` match arm is dispatched uniformly via `.await`, including
// handlers (config/presence-override/etc.) that are currently all-sync internally
// — kept `async fn` for that uniform call shape rather than splitting the match.
#![allow(clippy::unused_async)]

use clap::Parser;
use lucid::cli::{self, Cli, Command, ConfigCommand, PmCommand, PresenceCommand, TaskCommand};
use lucid::config::{Config, default_override_path};
use lucid::daemon::Daemon;
use lucid::presence::override_file::{OverrideFile, OverrideMode};
use lucid::presence::{self, PresenceMode, PresenceSourceList};
use lucid::tracker::{DecisionState, decision_label};
use std::path::PathBuf;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Start { foreground, config } => start(foreground, config).await,
        Command::Stop => stop().await,
        Command::Status { format, watch } => status(format, watch).await,
        Command::Show {
            worker_id,
            format,
            log_lines,
        } => show(&worker_id, format, log_lines).await,
        Command::Pm { command } => match command {
            PmCommand::Wake {
                respect_presence,
                dry_run,
                config,
            } => pm_wake(respect_presence, dry_run, config).await,
        },
        Command::Presence { command } => match command {
            PresenceCommand::Status { format, config } => presence_status(format, config).await,
            PresenceCommand::Override { mode, config } => presence_override(mode, config).await,
        },
        Command::Config { command } => match command {
            ConfigCommand::Validate { config } => config_validate(config).await,
            ConfigCommand::Show { config, format } => config_show(config, format).await,
        },
        Command::Task { command } => match command {
            TaskCommand::List { state, format, config } => task_list(state, format, config).await,
            TaskCommand::Approve { issue_id, config } => {
                task_set_decision(&issue_id, DecisionState::Approved, config).await
            }
            TaskCommand::Reject { issue_id, config } => {
                task_set_decision(&issue_id, DecisionState::Rejected, config).await
            }
            TaskCommand::DispatchNow { issue_id, config } => task_dispatch_now(&issue_id, config).await,
        },
    }
}

/// `./lucid.toml`, or `$XDG_CONFIG_HOME/lucid/config.toml` (falling back to
/// `~/.config/lucid/config.toml`) when no path is given — see docs/CLI.md.
fn resolve_config_path(explicit: Option<PathBuf>) -> PathBuf {
    if let Some(path) = explicit {
        return path;
    }
    let local = PathBuf::from("lucid.toml");
    if local.exists() {
        return local;
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("lucid/config.toml");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".config/lucid/config.toml");
    }
    local
}

fn override_file_for(config: &Config) -> OverrideFile {
    let path = config
        .presence
        .override_path
        .clone()
        .unwrap_or_else(default_override_path);
    OverrideFile::new(path)
}

/// No automatic presence sources are composed by default: `logind`'s real D-Bus
/// wiring isn't implemented yet (`todo!()` in `presence::logind`, and even once
/// built it's known-broken on WSL2 — see docs/wiki/architecture/presence-detection.md).
/// An empty source list means the daemon only ever goes autonomous via an explicit
/// `lucid presence override autonomous` — a deliberately conservative default, not
/// an oversight.
fn default_presence_sources() -> PresenceSourceList {
    PresenceSourceList::new(vec![])
}

async fn start(foreground: bool, config: Option<PathBuf>) -> anyhow::Result<()> {
    if !foreground {
        anyhow::bail!(
            "detached mode isn't implemented yet (no systemd unit / IPC — see docs/CLI.md § Not yet designed); pass --foreground"
        );
    }
    let config = Config::load(&resolve_config_path(config))?;
    let tracker = lucid::tracker::build(&config.tracker)?;
    let daemon = Daemon::new(tracker, default_presence_sources(), &config);
    daemon.run_foreground().await
}

async fn stop() -> anyhow::Result<()> {
    anyhow::bail!(
        "not implemented: `lucid stop` needs a control socket or PID+signal IPC, not yet designed — see docs/CLI.md § Not yet designed. Use Ctrl-C on a foreground `lucid start` instead."
    )
}

async fn status(_format: cli::OutputFormat, _watch: bool) -> anyhow::Result<()> {
    anyhow::bail!(
        "not implemented: `lucid status` needs to query a running daemon over IPC, not yet designed — see docs/CLI.md § Not yet designed."
    )
}

async fn show(
    _worker_id: &str,
    _format: cli::OutputFormat,
    _log_lines: u32,
) -> anyhow::Result<()> {
    anyhow::bail!(
        "not implemented: `lucid show` needs to query a running daemon over IPC, not yet designed — see docs/CLI.md § Not yet designed."
    )
}

async fn pm_wake(respect_presence: bool, dry_run: bool, config: Option<PathBuf>) -> anyhow::Result<()> {
    let config = Config::load(&resolve_config_path(config))?;

    if respect_presence {
        let sources = default_presence_sources();
        let override_file = override_file_for(&config);
        let idle_threshold = Duration::from_secs(u64::from(config.presence.idle_threshold_minutes) * 60);
        let mode = presence::resolve(&sources, &override_file, idle_threshold).await?;
        if mode != PresenceMode::Autonomous {
            println!("presence mode is Active — skipping (pass without --respect-presence to bypass the gate)");
            return Ok(());
        }
    }

    let tracker = lucid::tracker::build(&config.tracker)?;
    let outcome = lucid::pm::wake(
        tracker.as_ref(),
        &config.harness_profiles,
        &config.observability,
        &config.daemon.workdir,
        "keep the codebase healthy and close concrete, low-risk gaps",
        config.presence.proposal_cap_per_wake,
        Duration::from_secs(config.daemon.stall_timeout_secs),
        dry_run,
    )
    .await?;

    println!(
        "PM wake: {} proposed, {} filed, {} skipped as similar to existing issues",
        outcome.proposed.len(),
        outcome.filed.len(),
        outcome.skipped_similar.len()
    );
    for p in &outcome.proposed {
        println!("  - {}", p.title);
    }
    if dry_run {
        println!("(dry run — nothing was actually filed)");
    }
    Ok(())
}

async fn presence_status(format: cli::OutputFormat, config: Option<PathBuf>) -> anyhow::Result<()> {
    let config = Config::load(&resolve_config_path(config))?;
    let sources = default_presence_sources();
    let override_file = override_file_for(&config);
    let idle_threshold = Duration::from_secs(u64::from(config.presence.idle_threshold_minutes) * 60);
    let mode = presence::resolve(&sources, &override_file, idle_threshold).await?;
    let override_mode = override_file.read()?;
    let readings = sources.readings().await;

    match format {
        cli::OutputFormat::Json => {
            let payload = serde_json::json!({
                "mode": format!("{mode:?}"),
                "override": override_mode.as_str(),
                "sources": readings.iter().map(|(name, idle, since)| {
                    serde_json::json!({"name": name, "idle": idle, "idle_since_secs": since.map(|d| d.as_secs())})
                }).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        cli::OutputFormat::Table => {
            println!("MODE: {mode:?} (override: {})", override_mode.as_str());
            println!();
            if readings.is_empty() {
                println!("(no automatic presence sources configured — logind D-Bus wiring isn't implemented yet)");
            } else {
                println!("{:<20} {:<10} IDLE SINCE", "SOURCE", "IDLE");
                for (name, idle, since) in readings {
                    let since_desc = since.map_or_else(|| "-".to_string(), |d| format!("{}s", d.as_secs()));
                    println!("{name:<20} {idle:<10} {since_desc}");
                }
            }
        }
    }
    Ok(())
}

async fn presence_override(mode: cli::PresenceOverrideMode, config: Option<PathBuf>) -> anyhow::Result<()> {
    let config = Config::load(&resolve_config_path(config))?;
    let override_file = override_file_for(&config);
    let mode = match mode {
        cli::PresenceOverrideMode::Active => OverrideMode::Active,
        cli::PresenceOverrideMode::Autonomous => OverrideMode::Autonomous,
        cli::PresenceOverrideMode::Auto => OverrideMode::Auto,
    };
    override_file.write(mode)?;
    println!("presence override set to {}", mode.as_str());
    Ok(())
}

async fn config_validate(config: Option<PathBuf>) -> anyhow::Result<()> {
    let path = resolve_config_path(config);
    let config = Config::load(&path)?;
    lucid::tracker::build(&config.tracker)?;
    println!("{} is valid", path.display());
    Ok(())
}

async fn config_show(config: Option<PathBuf>, format: cli::ConfigFormat) -> anyhow::Result<()> {
    let config = Config::load(&resolve_config_path(config))?;
    // Nothing in `Config` holds a secret value today (`api_key_env` is an env var
    // *name*, not the key itself) — nothing to redact yet. If a field that could
    // ever hold a live credential is added, redact it here before printing.
    match format {
        cli::ConfigFormat::Toml => println!("{}", toml::to_string_pretty(&config)?),
        cli::ConfigFormat::Json => println!("{}", serde_json::to_string_pretty(&config)?),
    }
    Ok(())
}

fn task_state_to_decision(state: cli::TaskState) -> DecisionState {
    match state {
        cli::TaskState::Pending => DecisionState::Pending,
        cli::TaskState::Approved => DecisionState::Approved,
        cli::TaskState::Rejected => DecisionState::Rejected,
        cli::TaskState::Done => DecisionState::Done,
        cli::TaskState::NeedsReview => DecisionState::NeedsReview,
    }
}

async fn task_list(state: cli::TaskState, format: cli::OutputFormat, config: Option<PathBuf>) -> anyhow::Result<()> {
    let config = Config::load(&resolve_config_path(config))?;
    let tracker = lucid::tracker::build(&config.tracker)?;
    let decision = task_state_to_decision(state);
    let issues = tracker.query_by_label(decision_label(decision)).await?;

    match format {
        cli::OutputFormat::Json => {
            let payload: Vec<_> = issues
                .iter()
                .map(|i| serde_json::json!({"id": i.id, "title": i.title, "review": format!("{:?}", i.review)}))
                .collect();
            println!("{}", serde_json::to_string_pretty(&payload)?);
        }
        cli::OutputFormat::Table => {
            if issues.is_empty() {
                println!("(no issues in state {decision:?})");
            } else {
                println!("{:<14} {:<8} TITLE", "ID", "REVIEW");
                for i in &issues {
                    println!("{:<14} {:<8} {}", i.id, format!("{:?}", i.review), i.title);
                }
            }
        }
    }
    Ok(())
}

/// Writes a decision state via the same `TrackerAdapter::set_decision_state` the
/// tracker's own UI action would trigger — Linear's label, for the real backend —
/// not a second, lucid-side record of approval. See
/// docs/wiki/architecture/worker-completion.md.
async fn task_set_decision(issue_id: &str, state: DecisionState, config: Option<PathBuf>) -> anyhow::Result<()> {
    let config = Config::load(&resolve_config_path(config))?;
    let tracker = lucid::tracker::build(&config.tracker)?;
    tracker.set_decision_state(issue_id, state).await?;
    println!("{issue_id} -> {state:?}");
    Ok(())
}

/// Runs the exact dispatch-and-finalize path the daemon's tick loop would run for
/// this issue — on demand instead of on the next tick. Requires the issue already
/// be `Approved` in the tracker: this triggers *when* approved work runs, it never
/// decides *whether* it's allowed to.
async fn task_dispatch_now(issue_id: &str, config: Option<PathBuf>) -> anyhow::Result<()> {
    let config = Config::load(&resolve_config_path(config))?;
    let tracker = lucid::tracker::build(&config.tracker)?;

    let approved = tracker.query_by_label(decision_label(DecisionState::Approved)).await?;
    let issue = approved.into_iter().find(|i| i.id == issue_id).ok_or_else(|| {
        anyhow::anyhow!(
            "`{issue_id}` isn't in the Approved state — approve it first (`lucid task approve {issue_id}`)"
        )
    })?;

    let run = lucid::worker::dispatch_and_finalize(
        tracker.as_ref(),
        &issue,
        &config.harness_profiles,
        &config.observability,
        &config.daemon.workdir,
        Duration::from_secs(config.daemon.stall_timeout_secs),
        config.daemon.completion_mode,
        config.daemon.verify_cmd.as_deref(),
    )
    .await?;

    println!("{issue_id}: {:?}", run.phase);
    if let Some(err) = &run.last_error {
        println!("error: {err}");
    }
    Ok(())
}
