// Skeleton phase: nothing is wired together yet, so most types here are only
// referenced from `todo!()` bodies or not yet at all. Remove this once main's
// handlers actually construct and use them.
#![allow(dead_code)]

mod cli;
mod config;
mod harness;
mod presence;
mod state;
mod tracker;

use clap::Parser;
use cli::{Cli, Command, ConfigCommand, PmCommand, PresenceCommand};

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
            } => pm_wake(respect_presence, dry_run).await,
        },
        Command::Presence { command } => match command {
            PresenceCommand::Status { format } => presence_status(format).await,
            PresenceCommand::Override { mode } => presence_override(mode).await,
        },
        Command::Config { command } => match command {
            ConfigCommand::Validate { config } => config_validate(config).await,
            ConfigCommand::Show { config, format } => config_show(config, format).await,
        },
    }
}

async fn start(
    _foreground: bool,
    _config: Option<std::path::PathBuf>,
) -> anyhow::Result<()> {
    todo!("start presence watcher, reconciliation tick, PM wake scheduling")
}

async fn stop() -> anyhow::Result<()> {
    todo!("send graceful shutdown to a running daemon")
}

async fn status(_format: cli::OutputFormat, _watch: bool) -> anyhow::Result<()> {
    todo!("render running/blocked/retrying agents from state store")
}

async fn show(
    _worker_id: &str,
    _format: cli::OutputFormat,
    _log_lines: u32,
) -> anyhow::Result<()> {
    todo!("render one worker's full phase history and logs")
}

async fn pm_wake(_respect_presence: bool, _dry_run: bool) -> anyhow::Result<()> {
    todo!("run a PM gap-detection cycle on demand")
}

async fn presence_status(_format: cli::OutputFormat) -> anyhow::Result<()> {
    todo!("render presence mode and per-source readings")
}

async fn presence_override(_mode: cli::PresenceOverrideMode) -> anyhow::Result<()> {
    todo!("write the presence override state file")
}

async fn config_validate(_config: Option<std::path::PathBuf>) -> anyhow::Result<()> {
    todo!("load and validate the config file")
}

async fn config_show(
    _config: Option<std::path::PathBuf>,
    _format: cli::ConfigFormat,
) -> anyhow::Result<()> {
    todo!("load config, redact secrets, print in the requested format")
}
