// Every `Command` match arm is dispatched uniformly via `.await`, including
// handlers (config/presence-override/etc.) that are currently all-sync internally
// — kept `async fn` for that uniform call shape rather than splitting the match.
#![allow(clippy::unused_async)]

use clap::Parser;
use lucid::cli::{self, Cli, Command, ConfigCommand, PmCommand, PresenceCommand, TaskCommand};
use lucid::config::{Config, ProjectConfig, ProjectPointer, default_override_path};
use lucid::daemon::Daemon;
use lucid::presence::override_file::{OverrideFile, OverrideMode};
use lucid::presence::{self, PresenceMode, PresenceSourceList};
use lucid::tracker::{DecisionState, EffortEstimate, Proposal, ReviewMode};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Best-effort: a missing .env (the common case in prod/CI, where secrets come
    // from the real environment) isn't an error.
    let _ = dotenvy::dotenv();

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
        Command::Task { command } => match *command {
            TaskCommand::List {
                state,
                format,
                config,
                project,
            } => task_list(state, format, config, project).await,
            TaskCommand::Approve {
                issue_id,
                config,
                project,
            } => task_set_decision(&issue_id, DecisionState::Approved, config, project).await,
            TaskCommand::Reject {
                issue_id,
                config,
                project,
            } => task_set_decision(&issue_id, DecisionState::Rejected, config, project).await,
            TaskCommand::DispatchNow {
                issue_id,
                config,
                project,
            } => task_dispatch_now(&issue_id, config, project).await,
            TaskCommand::Create {
                title,
                summary,
                why_now,
                effort,
                risk_note,
                task_type,
                target_paths,
                acceptance_criteria,
                review,
                verify_cmd,
                config,
                project,
            } => {
                task_create(
                    title,
                    summary,
                    why_now,
                    effort,
                    risk_note,
                    task_type,
                    target_paths,
                    acceptance_criteria,
                    review,
                    verify_cmd,
                    config,
                    project,
                )
                .await
            }
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

/// A `[[projects]]` entry resolved against `--project`/cwd, plus its own
/// checked-in `lucid.project.toml`.
#[derive(Debug)]
struct ResolvedProject {
    name: String,
    path: PathBuf,
    project_config: ProjectConfig,
}

/// The name a project is addressed by on the CLI — the final component of its
/// pointer `path`, since `[[projects]]` entries don't carry a separate name
/// field (see docs/wiki/architecture/multi-project.md).
fn project_name(pointer: &ProjectPointer) -> String {
    pointer.path.file_name().map_or_else(
        || pointer.path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// Resolves which configured project a `task` subcommand targets — directory
/// detection by default, `--project <name>` to override. Never guesses: a cwd
/// matching zero or more than one configured project is a hard error listing
/// the configured names, and no state is persisted anywhere (recomputed fresh
/// every call). Repos with no `[[projects]]` configured keep today's
/// single-project behavior unchanged — `None` means "use `config.daemon.*`
/// directly", not "no project found".
///
/// # Errors
/// Returns an error if `--project` names an unconfigured project, if `cwd`
/// matches no configured project, if `cwd` matches more than one, or if a
/// matched project's own `lucid.project.toml` fails to load.
fn resolve_project(
    config: &Config,
    project_flag: Option<&str>,
    cwd: &Path,
) -> anyhow::Result<Option<ResolvedProject>> {
    if config.projects.is_empty() {
        if let Some(name) = project_flag {
            anyhow::bail!(
                "--project {name} was given, but no [[projects]] are configured in this lucid.toml"
            );
        }
        return Ok(None);
    }

    let names = || {
        config
            .projects
            .iter()
            .map(project_name)
            .collect::<Vec<_>>()
            .join(", ")
    };

    let pointer = if let Some(name) = project_flag {
        config
            .projects
            .iter()
            .find(|p| project_name(p) == name)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no configured project named `{name}` — configured: {}",
                    names()
                )
            })?
    } else {
        let cwd_canon = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
        let matches: Vec<&ProjectPointer> = config
            .projects
            .iter()
            .filter(|p| {
                let path_canon = p.path.canonicalize().unwrap_or_else(|_| p.path.clone());
                cwd_canon.starts_with(&path_canon)
            })
            .collect();

        match matches.as_slice() {
            [] => anyhow::bail!(
                "current directory doesn't match any configured project's workdir — pass --project <name> to select one; configured: {}",
                names()
            ),
            [single] => *single,
            _ => anyhow::bail!(
                "current directory matches more than one configured project — pass --project <name> to disambiguate; configured: {}",
                names()
            ),
        }
    };

    let project_config = ProjectConfig::load(&pointer.path)
        .map_err(|e| anyhow::anyhow!("project `{}`: {e}", project_name(pointer)))?;

    Ok(Some(ResolvedProject {
        name: project_name(pointer),
        path: pointer.path.clone(),
        project_config,
    }))
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

async fn show(_worker_id: &str, _format: cli::OutputFormat, _log_lines: u32) -> anyhow::Result<()> {
    anyhow::bail!(
        "not implemented: `lucid show` needs to query a running daemon over IPC, not yet designed — see docs/CLI.md § Not yet designed."
    )
}

async fn pm_wake(
    respect_presence: bool,
    dry_run: bool,
    config: Option<PathBuf>,
) -> anyhow::Result<()> {
    let config = Config::load(&resolve_config_path(config))?;

    if respect_presence {
        let sources = default_presence_sources();
        let override_file = override_file_for(&config);
        let idle_threshold =
            Duration::from_secs(u64::from(config.presence.idle_threshold_minutes) * 60);
        let mode = presence::resolve(&sources, &override_file, idle_threshold).await?;
        if mode != PresenceMode::Autonomous {
            println!(
                "presence mode is Active — skipping (pass without --respect-presence to bypass the gate)"
            );
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
    let idle_threshold =
        Duration::from_secs(u64::from(config.presence.idle_threshold_minutes) * 60);
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
                println!(
                    "(no automatic presence sources configured — logind D-Bus wiring isn't implemented yet)"
                );
            } else {
                println!("{:<20} {:<10} IDLE SINCE", "SOURCE", "IDLE");
                for (name, idle, since) in readings {
                    let since_desc =
                        since.map_or_else(|| "-".to_string(), |d| format!("{}s", d.as_secs()));
                    println!("{name:<20} {idle:<10} {since_desc}");
                }
            }
        }
    }
    Ok(())
}

async fn presence_override(
    mode: cli::PresenceOverrideMode,
    config: Option<PathBuf>,
) -> anyhow::Result<()> {
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
    for profile in &config.harness_profiles {
        if profile.unsandboxed {
            println!(
                "warning: profile `{}` runs unsandboxed (execution_backend = Local)",
                profile.name
            );
        }
    }
    config.validate_projects()?;
    if !config.projects.is_empty() {
        println!("{} project(s) valid", config.projects.len());
    }
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

/// Files a new proposal directly through the tracker adapter — the same write
/// path `pm::wake` uses, minus its `query_similar` dedup check: a human typing a
/// title explicitly isn't the runaway re-filing case dedup guards against, so
/// this always creates.
#[allow(clippy::too_many_arguments)]
async fn task_create(
    title: String,
    summary: Option<String>,
    why_now: Vec<String>,
    effort: cli::CliEffort,
    risk_note: String,
    task_type: String,
    target_paths: Vec<String>,
    acceptance_criteria: Vec<String>,
    review: cli::CliReviewMode,
    verify_cmd: Option<String>,
    config: Option<PathBuf>,
    project: Option<String>,
) -> anyhow::Result<()> {
    let config = Config::load(&resolve_config_path(config))?;
    resolve_project(&config, project.as_deref(), &std::env::current_dir()?)?;
    let tracker = lucid::tracker::build(&config.tracker)?;

    let review = match review {
        cli::CliReviewMode::Auto => ReviewMode::Auto,
        cli::CliReviewMode::Human => ReviewMode::Human,
        cli::CliReviewMode::Agent => ReviewMode::Agent,
    };
    let proposal = Proposal {
        summary: summary.unwrap_or_else(|| title.clone()),
        title,
        why_now,
        effort_estimate: match effort {
            cli::CliEffort::Small => EffortEstimate::Small,
            cli::CliEffort::Medium => EffortEstimate::Medium,
            cli::CliEffort::Large => EffortEstimate::Large,
        },
        risk_note,
        task_type,
        target_paths,
        acceptance_criteria,
        research_ref: None,
        review,
        verify_cmd,
    };

    let id = tracker.create_proposal(&proposal).await?;
    println!("{id}");
    Ok(())
}

async fn task_list(
    state: cli::TaskState,
    format: cli::OutputFormat,
    config: Option<PathBuf>,
    project: Option<String>,
) -> anyhow::Result<()> {
    let config = Config::load(&resolve_config_path(config))?;
    resolve_project(&config, project.as_deref(), &std::env::current_dir()?)?;
    let tracker = lucid::tracker::build(&config.tracker)?;
    let decision = task_state_to_decision(state);
    let issues = tracker.query_by_decision_state(decision).await?;

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
/// tracker's own UI action would trigger — moving the issue's real ticket state
/// for the Linear backend, not a second, lucid-side record of approval. See
/// docs/wiki/architecture/worker-completion.md.
async fn task_set_decision(
    issue_id: &str,
    state: DecisionState,
    config: Option<PathBuf>,
    project: Option<String>,
) -> anyhow::Result<()> {
    let config = Config::load(&resolve_config_path(config))?;
    resolve_project(&config, project.as_deref(), &std::env::current_dir()?)?;
    let tracker = lucid::tracker::build(&config.tracker)?;
    tracker.set_decision_state(issue_id, state).await?;
    println!("{issue_id} -> {state:?}");
    Ok(())
}

/// Runs the exact dispatch-and-finalize path the daemon's tick loop would run for
/// this issue — on demand instead of on the next tick. Requires the issue already
/// be `Approved` in the tracker: this triggers *when* approved work runs, it never
/// decides *whether* it's allowed to.
async fn task_dispatch_now(
    issue_id: &str,
    config: Option<PathBuf>,
    project: Option<String>,
) -> anyhow::Result<()> {
    let config = Config::load(&resolve_config_path(config))?;
    let resolved = resolve_project(&config, project.as_deref(), &std::env::current_dir()?)?;
    let tracker = lucid::tracker::build(&config.tracker)?;

    let (workdir, base_branch, verify_cmd) = resolved.as_ref().map_or(
        (
            config.daemon.workdir.clone(),
            config.daemon.base_branch.clone(),
            config.daemon.verify_cmd.clone(),
        ),
        |p| {
            (
                p.path.clone(),
                p.project_config.base_branch.clone(),
                p.project_config
                    .verify_cmd
                    .clone()
                    .or_else(|| config.daemon.verify_cmd.clone()),
            )
        },
    );

    let approved = tracker
        .query_by_decision_state(DecisionState::Approved)
        .await?;
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
        &workdir,
        &config.daemon.worktree_root,
        &base_branch,
        Duration::from_secs(config.daemon.stall_timeout_secs),
        verify_cmd.as_deref(),
    )
    .await?;

    if let Some(p) = &resolved {
        println!("[{}] {issue_id}: {:?}", p.name, run.phase);
    } else {
        println!("{issue_id}: {:?}", run.phase);
    }
    if let Some(err) = &run.last_error {
        println!("error: {err}");
    }
    Ok(())
}

#[cfg(test)]
mod project_resolution_tests {
    use super::{Config, resolve_project};
    use lucid::config::PROJECT_CONFIG_FILENAME;
    use std::path::PathBuf;

    fn write_project(base_branch: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("lucid-cli-project-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(PROJECT_CONFIG_FILENAME),
            format!(r#"base_branch = "{base_branch}""#),
        )
        .unwrap();
        dir
    }

    fn write_config(project_paths: &[&std::path::Path]) -> Config {
        let mut projects_toml = String::new();
        for p in project_paths {
            use std::fmt::Write;
            let _ = writeln!(projects_toml, "[[projects]]\npath = \"{}\"", p.display());
        }
        let toml = format!(
            r#"
            [[harness_profiles]]
            name = "claude-subscription"
            kind = "ClaudeCode"
            cmd = "claude"
            args = ["-p"]
            auth_mode = "Subscription"
            priority = 1

            [tracker]
            backend = "file"
            file_path = "/tmp/lucid-cli-test-tracker.json"

            [presence]
            idle_threshold_minutes = 20
            proposal_cap_per_wake = 3

            [observability]
            otlp_endpoint = "http://localhost:4317"
            trace_ui_base_url = "http://localhost:6006"

            {projects_toml}
        "#
        );
        let path = std::env::temp_dir().join(format!(
            "lucid-cli-config-test-{}.toml",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, toml).unwrap();
        let config = Config::load(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        config
    }

    #[test]
    fn no_projects_configured_resolves_to_none_regardless_of_cwd() {
        let config = write_config(&[]);
        let resolved = resolve_project(&config, None, &std::env::temp_dir()).unwrap();
        assert!(resolved.is_none());
    }

    #[test]
    fn directory_detection_resolves_the_right_project() {
        let project_a = write_project("main");
        let project_b = write_project("trunk");
        let config = write_config(&[&project_a, &project_b]);

        let cwd = project_a.join("src");
        let resolved = resolve_project(&config, None, &cwd).unwrap().unwrap();
        assert_eq!(resolved.path, project_a);
        assert_eq!(resolved.project_config.base_branch, "main");

        let _ = std::fs::remove_dir_all(&project_a);
        let _ = std::fs::remove_dir_all(&project_b);
    }

    #[test]
    fn unmatched_cwd_produces_a_clear_error_listing_configured_projects() {
        let project_a = write_project("main");
        let project_b = write_project("trunk");
        let config = write_config(&[&project_a, &project_b]);

        let unrelated_cwd =
            std::env::temp_dir().join(format!("lucid-cli-unrelated-{}", uuid::Uuid::new_v4()));
        let err = resolve_project(&config, None, &unrelated_cwd).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("doesn't match any configured project"));
        assert!(message.contains(&super::project_name(&config.projects[0])));
        assert!(message.contains(&super::project_name(&config.projects[1])));

        let _ = std::fs::remove_dir_all(&project_a);
        let _ = std::fs::remove_dir_all(&project_b);
    }

    #[test]
    fn unknown_project_flag_produces_a_clear_error() {
        let project_a = write_project("main");
        let config = write_config(&[&project_a]);

        let err =
            resolve_project(&config, Some("does-not-exist"), &std::env::temp_dir()).unwrap_err();
        assert!(err.to_string().contains("no configured project named"));

        let _ = std::fs::remove_dir_all(&project_a);
    }

    #[test]
    fn explicit_project_flag_overrides_directory_detection() {
        let project_a = write_project("main");
        let project_b = write_project("trunk");
        let config = write_config(&[&project_a, &project_b]);
        let project_b_name = super::project_name(&config.projects[1]);

        // cwd sits inside project_a, but --project explicitly picks project_b.
        let cwd = project_a.join("src");
        let resolved = resolve_project(&config, Some(&project_b_name), &cwd)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.path, project_b);
        assert_eq!(resolved.project_config.base_branch, "trunk");

        let _ = std::fs::remove_dir_all(&project_a);
        let _ = std::fs::remove_dir_all(&project_b);
    }
}
