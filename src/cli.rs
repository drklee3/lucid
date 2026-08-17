//! CLI surface — matches docs/CLI.md. Parsing only; handlers live in main.rs.

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "lucid", about = "Presence-aware autonomous development orchestrator")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Start the orchestrator daemon
    Start {
        #[arg(long)]
        foreground: bool,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Stop a running daemon
    Stop,
    /// List running/blocked/retrying agents
    Status {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
        #[arg(long)]
        watch: bool,
    },
    /// Inspect one worker's session in detail
    Show {
        worker_id: String,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
        #[arg(long, default_value_t = 50)]
        log_lines: u32,
    },
    /// PM gap-detection commands
    Pm {
        #[command(subcommand)]
        command: PmCommand,
    },
    /// Presence-source commands
    Presence {
        #[command(subcommand)]
        command: PresenceCommand,
    },
    /// Config commands
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },
    /// Inspect/act on tracker items — a terminal convenience over the tracker's
    /// own UI (Linear), not a second source of truth: every subcommand goes
    /// through the same `TrackerAdapter` the daemon itself uses. See
    /// docs/wiki/architecture/worker-completion.md.
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
}

#[derive(Subcommand, Debug)]
pub enum TaskCommand {
    /// List issues in a given decision state (default: approved)
    List {
        #[arg(long, value_enum, default_value_t = TaskState::Approved)]
        state: TaskState,
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Approve an issue for dispatch
    Approve {
        issue_id: String,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Reject an issue
    Reject {
        issue_id: String,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Dispatch one already-`Approved` issue right now, bypassing the tick
    /// interval and presence gate — the same dispatch path the daemon's regular
    /// tick uses, just triggered on demand instead of on a timer.
    DispatchNow {
        issue_id: String,
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum TaskState {
    Pending,
    Approved,
    Rejected,
    Done,
    NeedsReview,
}

#[derive(Subcommand, Debug)]
pub enum PmCommand {
    /// Manually trigger a PM gap-detection wake cycle
    Wake {
        #[arg(long)]
        respect_presence: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum PresenceCommand {
    /// Show current presence mode and per-source readings
    Status {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Force or clear the presence override
    Override {
        mode: PresenceOverrideMode,
        #[arg(long)]
        config: Option<PathBuf>,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum PresenceOverrideMode {
    Active,
    Autonomous,
    Auto,
}

#[derive(Subcommand, Debug)]
pub enum ConfigCommand {
    /// Validate the config file without starting anything
    Validate {
        #[arg(long)]
        config: Option<PathBuf>,
    },
    /// Print resolved config (secrets redacted)
    Show {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = ConfigFormat::Toml)]
        format: ConfigFormat,
    },
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum OutputFormat {
    Table,
    Json,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum ConfigFormat {
    Toml,
    Json,
}
