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
}

#[derive(Subcommand, Debug)]
pub enum PmCommand {
    /// Manually trigger a PM gap-detection wake cycle
    Wake {
        #[arg(long)]
        respect_presence: bool,
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand, Debug)]
pub enum PresenceCommand {
    /// Show current presence mode and per-source readings
    Status {
        #[arg(long, value_enum, default_value_t = OutputFormat::Table)]
        format: OutputFormat,
    },
    /// Force or clear the presence override
    Override { mode: PresenceOverrideMode },
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
