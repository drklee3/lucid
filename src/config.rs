//! Config loading — harness profiles, tracker settings, presence thresholds, and
//! the daemon's own tick/timeout knobs.

use crate::harness::{ExecutionBackend, HarnessProfile};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize, Deserialize)]
pub struct Config {
    pub harness_profiles: Vec<HarnessProfile>,
    pub tracker: TrackerConfig,
    pub presence: PresenceConfig,
    pub observability: ObservabilityConfig,
    #[serde(default)]
    pub daemon: DaemonConfig,
    /// Repos this daemon instance watches, each a pointer to a checked-out
    /// working copy rather than a full config block — see
    /// docs/wiki/architecture/multi-project.md. Empty by default so today's
    /// single-project `lucid.toml` shape keeps loading unchanged.
    #[serde(default)]
    pub projects: Vec<ProjectPointer>,
}

/// One entry in `[[projects]]` — just the path to a repo this daemon watches.
/// The repo-specific settings (tracker project key, `verify_cmd`,
/// `base_branch`) live in that repo's own checked-in [`ProjectConfig`], not
/// here, so a project's settings travel with its code (Symphony's
/// `WORKFLOW.md` pattern — see docs/wiki/architecture/symphony-patterns.md).
#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectPointer {
    pub path: PathBuf,
}

/// Filename of the per-repo checked-in config file each `[[projects]]` entry's
/// `path` is expected to contain.
pub const PROJECT_CONFIG_FILENAME: &str = "lucid.project.toml";

/// Where a project's tickets can originate. `OperatorOnly` — the operator's own
/// CLI (`lucid task create`) or direct tracker approval — carries the same trust
/// level as running lucid locally. `AcceptsExternal` means anyone who can create a
/// tracker item (e.g. a teammate's Discord message) can produce a `Pending`
/// proposal without the operator writing it, which is the case
/// `Config::validate_trust_routing` requires a sandboxed harness profile for —
/// see docs/wiki/architecture/sandboxed-execution.md's trust-routing section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum TicketSource {
    #[default]
    OperatorOnly,
    AcceptsExternal,
}

/// The repo-owned half of per-project config — checked into and versioned
/// with the project's own code, read from `PROJECT_CONFIG_FILENAME` at the
/// project's pointer path. See docs/wiki/architecture/multi-project.md.
#[derive(Debug, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// Linear project name/slug/id to scope issues to — same meaning as
    /// `TrackerConfig::project_key`, but declared by the repo rather than the
    /// operator's central config.
    #[serde(default)]
    pub project_key: Option<String>,
    /// This project's deterministic verify step (see
    /// docs/wiki/architecture/worker-completion.md). `None` leaves the review
    /// agent to infer its own command.
    #[serde(default)]
    pub verify_cmd: Option<String>,
    /// Branch this project's dispatch worktrees are created from, and PRs
    /// target. Defaults to `"main"`.
    #[serde(default = "default_base_branch")]
    pub base_branch: String,
    /// Who can put a ticket in front of this project's dispatch loop. Defaults to
    /// `OperatorOnly` — a project only starts requiring a sandboxed harness
    /// profile once it opts into `AcceptsExternal`.
    #[serde(default)]
    pub ticket_source: TicketSource,
}

impl ProjectConfig {
    /// # Errors
    /// Returns an error if `PROJECT_CONFIG_FILENAME` under `project_path` can't
    /// be read or doesn't parse as valid TOML matching this shape.
    pub fn load(project_path: &Path) -> anyhow::Result<Self> {
        let config_path = project_path.join(PROJECT_CONFIG_FILENAME);
        let data = std::fs::read_to_string(&config_path).map_err(|e| {
            anyhow::anyhow!("reading project config at {}: {e}", config_path.display())
        })?;
        toml::from_str(&data).map_err(|e| {
            anyhow::anyhow!("parsing project config at {}: {e}", config_path.display())
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackerConfig {
    /// `"file"` (local JSON, no credentials — see `tracker::file::FileTracker`) or
    /// `"linear"` (real Linear GraphQL — see `tracker::linear::LinearAdapter`).
    pub backend: String,
    /// Name of the env var holding the tracker's API key — for the `linear`
    /// backend, `LINEAR_API_KEY` (see docs/wiki/architecture/tracker-adapter.md).
    /// Unused by the `file` backend.
    #[serde(default)]
    pub api_key_env: Option<String>,
    /// Linear's short team key (e.g. `ENG`), not its UUID. `LinearAdapter` needs
    /// this to resolve team/label ids — required when `backend = "linear"`,
    /// unused by other backends.
    #[serde(default)]
    pub team_key: Option<String>,
    /// Linear project name/slug/id to scope issues to within `team_key`. Optional —
    /// Linear issues don't require a project, and omitting this leaves lucid
    /// operating across the whole team as before.
    #[serde(default)]
    pub project_key: Option<String>,
    /// Where `FileTracker` persists its JSON store — required when
    /// `backend = "file"`, unused by other backends.
    #[serde(default)]
    pub file_path: Option<PathBuf>,
    /// Label (e.g. `"lucid"`) every query `LinearAdapter` uses to find its own
    /// work additionally requires, on top of `team_key`/`project_key` scoping —
    /// see docs/wiki/architecture/tracker-adapter.md. `None` preserves today's
    /// behavior (team/project scoping only, no label filter). Like `team_key`,
    /// lucid never creates this label: a missing one is a workspace-setup error.
    #[serde(default)]
    pub managed_label: Option<String>,
}

/// Where dispatched harnesses send `OTel` traces/logs — see
/// docs/wiki/architecture/trace-correlation.md.
#[derive(Debug, Serialize, Deserialize)]
pub struct ObservabilityConfig {
    pub otlp_endpoint: String,
    /// Opt-in prompt/tool-content capture — off by default on both harnesses, since
    /// it's the point at which the trace store starts holding sensitive content.
    #[serde(default)]
    pub log_prompts: bool,
    /// Base URL of the trace UI (e.g. Phoenix's `http://localhost:6006`), used to
    /// build the `lucid.dispatch_id`-filtered link posted back to the tracker item.
    pub trace_ui_base_url: String,
    /// Phoenix project slug/id to link into, if the backend is project-scoped.
    /// `None` falls back to `"default"` — Phoenix's default project.
    #[serde(default)]
    pub trace_ui_project_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PresenceConfig {
    /// Minutes of sustained idle before flipping to autonomous mode.
    pub idle_threshold_minutes: u32,
    /// Max proposals a PM wake cycle may file.
    pub proposal_cap_per_wake: u32,
    /// State file the explicit override layer reads/writes (see
    /// docs/wiki/architecture/presence-detection.md). Defaults to
    /// `$XDG_STATE_HOME/lucid/presence-override` (or `~/.local/state/...`) when
    /// unset.
    #[serde(default)]
    pub override_path: Option<PathBuf>,
}

/// The daemon's own tick/timeout knobs — separate from `PresenceConfig` because
/// these govern the reconciliation loop's mechanics, not presence detection
/// itself.
#[derive(Debug, Serialize, Deserialize)]
pub struct DaemonConfig {
    /// How often the reconciliation loop checks presence + dispatches
    /// newly-approved issues.
    #[serde(default = "default_tick_interval_secs")]
    pub tick_interval_secs: u64,
    /// Hard per-dispatch wall-clock limit before a harness process is killed and
    /// the run marked `TimedOut` (see `harness::DispatchRequest::timeout`).
    #[serde(default = "default_stall_timeout_secs")]
    pub stall_timeout_secs: u64,
    /// Minimum time between PM gap-detection wake cycles while autonomous.
    #[serde(default = "default_pm_wake_interval_mins")]
    pub pm_wake_interval_mins: u64,
    /// The main repo checkout — every dispatch's worktree branches off
    /// `base_branch`'s tip here, and this is also where `gh pr create`/`gh pr
    /// merge` run from. Defaults to the current directory.
    #[serde(default = "default_workdir")]
    pub workdir: PathBuf,
    /// Branch each dispatch's worktree is created from, and PRs target — see
    /// docs/wiki/architecture/worker-completion.md. Defaults to `"main"`.
    #[serde(default = "default_base_branch")]
    pub base_branch: String,
    /// Where per-issue worktrees are created (see `worktree::create`) —
    /// deliberately outside `workdir` so they never show up in the main repo's own
    /// `git status`. Defaults to a `lucid-worktrees` directory under the system
    /// temp dir.
    #[serde(default = "default_worktree_root")]
    pub worktree_root: PathBuf,
    /// Repo-wide default for `ReviewMode::Agent`'s deterministic verify step (see
    /// docs/wiki/architecture/worker-completion.md) — the common case is one
    /// command that's true for every task in a repo (e.g. `cargo test`), same as
    /// CI, so this is the primary way to set it. A `Proposal.verify_cmd` on a
    /// specific task overrides this one, for the exception (a docs-only task, a
    /// monorepo task scoped to one package) rather than the rule. `None` (the
    /// default) leaves the review agent to infer its own command per task.
    #[serde(default)]
    pub verify_cmd: Option<String>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            tick_interval_secs: default_tick_interval_secs(),
            stall_timeout_secs: default_stall_timeout_secs(),
            pm_wake_interval_mins: default_pm_wake_interval_mins(),
            workdir: default_workdir(),
            base_branch: default_base_branch(),
            worktree_root: default_worktree_root(),
            verify_cmd: None,
        }
    }
}

fn default_tick_interval_secs() -> u64 {
    30
}

fn default_stall_timeout_secs() -> u64 {
    600
}

fn default_pm_wake_interval_mins() -> u64 {
    60
}

fn default_workdir() -> PathBuf {
    PathBuf::from(".")
}

fn default_base_branch() -> String {
    "main".to_string()
}

fn default_worktree_root() -> PathBuf {
    std::env::temp_dir().join("lucid-worktrees")
}

/// `$XDG_STATE_HOME/lucid/presence-override`, falling back to
/// `~/.local/state/lucid/presence-override` when `XDG_STATE_HOME` and `HOME` are
/// both unset this just returns a relative fallback rather than panicking, since
/// presence override is a convenience layer, not something worth crashing over.
#[must_use]
pub fn default_override_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        return PathBuf::from(xdg).join("lucid/presence-override");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".local/state/lucid/presence-override");
    }
    PathBuf::from(".lucid-presence-override")
}

impl Config {
    /// # Errors
    /// Returns an error if the file can't be read or doesn't parse as valid TOML
    /// matching this shape.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let data = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading config at {}: {e}", path.display()))?;
        let config: Self = toml::from_str(&data)
            .map_err(|e| anyhow::anyhow!("parsing config at {}: {e}", path.display()))?;
        for profile in &config.harness_profiles {
            profile.validate()?;
        }
        Ok(config)
    }

    /// Resolves and validates every configured project's own
    /// `PROJECT_CONFIG_FILENAME`.
    ///
    /// # Errors
    /// Returns an error naming the offending project's path if that project's
    /// config file is missing or fails to parse.
    pub fn validate_projects(&self) -> anyhow::Result<Vec<ProjectConfig>> {
        self.projects
            .iter()
            .map(|project| {
                ProjectConfig::load(&project.path)
                    .map_err(|e| anyhow::anyhow!("project `{}`: {e}", project.path.display()))
            })
            .collect()
    }

    /// Refuses to pass if any project's resolved `ProjectConfig` accepts external
    /// ticket sources while no `harness_profiles` entry runs
    /// `ExecutionBackend::Sandboxed` — a project that never leaves `OperatorOnly`
    /// is exempt, since this is a rail for external-trigger risk specifically, not
    /// a blanket sandboxing mandate. See docs/wiki/architecture/
    /// sandboxed-execution.md's trust-routing section. `projects` must be the
    /// output of `validate_projects` for `self` (same order as `self.projects`).
    ///
    /// # Errors
    /// Returns an error naming the offending project's path if it accepts
    /// external ticket sources and no configured harness profile is `Sandboxed`.
    pub fn validate_trust_routing(&self, projects: &[ProjectConfig]) -> anyhow::Result<()> {
        let has_sandboxed_profile = self
            .harness_profiles
            .iter()
            .any(|profile| profile.execution_backend == ExecutionBackend::Sandboxed);
        if has_sandboxed_profile {
            return Ok(());
        }
        for (pointer, project) in self.projects.iter().zip(projects) {
            if project.ticket_source == TicketSource::AcceptsExternal {
                anyhow::bail!(
                    "project `{}` accepts external ticket sources but no harness profile has \
                     execution_backend = \"Sandboxed\" configured — see \
                     docs/wiki/architecture/sandboxed-execution.md",
                    pointer.path.display()
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_a_minimal_config() {
        let toml = r#"
            [[harness_profiles]]
            name = "claude-subscription"
            kind = "ClaudeCode"
            cmd = "claude"
            args = ["-p"]
            auth_mode = "Subscription"
            priority = 1

            [tracker]
            backend = "file"
            file_path = "/tmp/lucid-test-tracker.json"

            [presence]
            idle_threshold_minutes = 20
            proposal_cap_per_wake = 3

            [observability]
            otlp_endpoint = "http://localhost:4317"
            trace_ui_base_url = "http://localhost:6006"
        "#;
        let path =
            std::env::temp_dir().join(format!("lucid-config-test-{}.toml", uuid::Uuid::new_v4()));
        std::fs::write(&path, toml).unwrap();

        let config = Config::load(&path).unwrap();
        assert_eq!(config.harness_profiles.len(), 1);
        assert_eq!(config.tracker.backend, "file");
        assert_eq!(config.daemon.tick_interval_secs, 30);
        assert_eq!(
            config.harness_profiles[0].execution_backend,
            crate::harness::ExecutionBackend::Sandboxed
        );
        assert!(!config.harness_profiles[0].unsandboxed);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_errors_instead_of_panicking() {
        let err = Config::load(Path::new("/nonexistent/lucid.toml")).unwrap_err();
        assert!(err.to_string().contains("reading config"));
    }

    #[test]
    fn loads_a_single_project_config_unchanged_from_todays_shape() {
        let toml = r#"
            [[harness_profiles]]
            name = "claude-subscription"
            kind = "ClaudeCode"
            cmd = "claude"
            args = ["-p"]
            auth_mode = "Subscription"
            priority = 1

            [tracker]
            backend = "file"
            file_path = "/tmp/lucid-test-tracker.json"

            [presence]
            idle_threshold_minutes = 20
            proposal_cap_per_wake = 3

            [observability]
            otlp_endpoint = "http://localhost:4317"
            trace_ui_base_url = "http://localhost:6006"
        "#;
        let path =
            std::env::temp_dir().join(format!("lucid-config-test-{}.toml", uuid::Uuid::new_v4()));
        std::fs::write(&path, toml).unwrap();

        let config = Config::load(&path).unwrap();
        assert!(config.projects.is_empty());
        assert!(config.validate_projects().unwrap().is_empty());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn loads_a_multi_project_config_and_resolves_each_projects_file() {
        let project_dir =
            std::env::temp_dir().join(format!("lucid-project-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(
            project_dir.join(PROJECT_CONFIG_FILENAME),
            r#"
                project_key = "ENG-123"
                verify_cmd = "cargo test"
                base_branch = "trunk"
            "#,
        )
        .unwrap();

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
            file_path = "/tmp/lucid-test-tracker.json"

            [presence]
            idle_threshold_minutes = 20
            proposal_cap_per_wake = 3

            [observability]
            otlp_endpoint = "http://localhost:4317"
            trace_ui_base_url = "http://localhost:6006"

            [[projects]]
            path = "{}"
        "#,
            project_dir.display()
        );
        let path =
            std::env::temp_dir().join(format!("lucid-config-test-{}.toml", uuid::Uuid::new_v4()));
        std::fs::write(&path, toml).unwrap();

        let config = Config::load(&path).unwrap();
        assert_eq!(config.projects.len(), 1);
        let projects = config.validate_projects().unwrap();
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].project_key.as_deref(), Some("ENG-123"));
        assert_eq!(projects[0].verify_cmd.as_deref(), Some("cargo test"));
        assert_eq!(projects[0].base_branch, "trunk");

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&project_dir);
    }

    #[test]
    fn missing_project_config_file_produces_a_clear_per_project_error() {
        let project_dir =
            std::env::temp_dir().join(format!("lucid-project-test-{}", uuid::Uuid::new_v4()));
        // Deliberately not creating the directory / config file.

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
            file_path = "/tmp/lucid-test-tracker.json"

            [presence]
            idle_threshold_minutes = 20
            proposal_cap_per_wake = 3

            [observability]
            otlp_endpoint = "http://localhost:4317"
            trace_ui_base_url = "http://localhost:6006"

            [[projects]]
            path = "{}"
        "#,
            project_dir.display()
        );
        let path =
            std::env::temp_dir().join(format!("lucid-config-test-{}.toml", uuid::Uuid::new_v4()));
        std::fs::write(&path, toml).unwrap();

        let config = Config::load(&path).unwrap();
        let err = config.validate_projects().unwrap_err();
        let message = err.to_string();
        assert!(message.contains(&project_dir.display().to_string()));
        assert!(message.contains("reading project config"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn malformed_project_config_file_produces_a_clear_per_project_error() {
        let project_dir =
            std::env::temp_dir().join(format!("lucid-project-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(
            project_dir.join(PROJECT_CONFIG_FILENAME),
            "not valid toml =",
        )
        .unwrap();

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
            file_path = "/tmp/lucid-test-tracker.json"

            [presence]
            idle_threshold_minutes = 20
            proposal_cap_per_wake = 3

            [observability]
            otlp_endpoint = "http://localhost:4317"
            trace_ui_base_url = "http://localhost:6006"

            [[projects]]
            path = "{}"
        "#,
            project_dir.display()
        );
        let path =
            std::env::temp_dir().join(format!("lucid-config-test-{}.toml", uuid::Uuid::new_v4()));
        std::fs::write(&path, toml).unwrap();

        let config = Config::load(&path).unwrap();
        let err = config.validate_projects().unwrap_err();
        let message = err.to_string();
        assert!(message.contains(&project_dir.display().to_string()));
        assert!(message.contains("parsing project config"));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&project_dir);
    }

    fn write_profile_only_config(extra_profile_lines: &str) -> PathBuf {
        let toml = format!(
            r#"
            [[harness_profiles]]
            name = "claude-local"
            kind = "ClaudeCode"
            cmd = "claude"
            args = ["-p"]
            auth_mode = "Subscription"
            priority = 1
            {extra_profile_lines}

            [tracker]
            backend = "file"
            file_path = "/tmp/lucid-test-tracker.json"

            [presence]
            idle_threshold_minutes = 20
            proposal_cap_per_wake = 3

            [observability]
            otlp_endpoint = "http://localhost:4317"
            trace_ui_base_url = "http://localhost:6006"
        "#
        );
        let path =
            std::env::temp_dir().join(format!("lucid-config-test-{}.toml", uuid::Uuid::new_v4()));
        std::fs::write(&path, toml).unwrap();
        path
    }

    #[test]
    fn explicit_local_backend_with_opt_out_is_accepted() {
        let path = write_profile_only_config(
            r#"execution_backend = "Local"
            unsandboxed = true"#,
        );

        let config = Config::load(&path).unwrap();
        assert_eq!(
            config.harness_profiles[0].execution_backend,
            crate::harness::ExecutionBackend::Local
        );
        assert!(config.harness_profiles[0].unsandboxed);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn local_backend_without_opt_out_is_rejected() {
        let path = write_profile_only_config(r#"execution_backend = "Local""#);

        let err = Config::load(&path).unwrap_err();
        assert!(err.to_string().contains("unsandboxed = true"));

        let _ = std::fs::remove_file(&path);
    }

    /// Writes a top-level config with one `[[projects]]` entry whose
    /// `lucid.project.toml` sets `ticket_source`, plus harness profiles governed
    /// by `harness_execution_backend_lines` (e.g. `execution_backend = "Local"\nunsandboxed = true`,
    /// or `""` for the all-`Sandboxed`-default case). Returns the top-level config
    /// path and the project dir, both left on disk for the caller to clean up.
    fn write_trust_routing_config(
        ticket_source_line: &str,
        harness_execution_backend_lines: &str,
    ) -> (PathBuf, PathBuf) {
        let project_dir =
            std::env::temp_dir().join(format!("lucid-project-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&project_dir).unwrap();
        std::fs::write(
            project_dir.join(PROJECT_CONFIG_FILENAME),
            format!(
                r#"
                project_key = "ENG-123"
                {ticket_source_line}
                "#
            ),
        )
        .unwrap();

        let toml = format!(
            r#"
            [[harness_profiles]]
            name = "claude-profile"
            kind = "ClaudeCode"
            cmd = "claude"
            args = ["-p"]
            auth_mode = "Subscription"
            priority = 1
            {harness_execution_backend_lines}

            [tracker]
            backend = "file"
            file_path = "/tmp/lucid-test-tracker.json"

            [presence]
            idle_threshold_minutes = 20
            proposal_cap_per_wake = 3

            [observability]
            otlp_endpoint = "http://localhost:4317"
            trace_ui_base_url = "http://localhost:6006"

            [[projects]]
            path = "{}"
        "#,
            project_dir.display()
        );
        let path =
            std::env::temp_dir().join(format!("lucid-config-test-{}.toml", uuid::Uuid::new_v4()));
        std::fs::write(&path, toml).unwrap();
        (path, project_dir)
    }

    #[test]
    fn external_accepting_project_with_sandboxed_profile_passes() {
        let (path, project_dir) = write_trust_routing_config(
            r#"ticket_source = "accepts-external""#,
            "", // default execution_backend = Sandboxed
        );

        let config = Config::load(&path).unwrap();
        let projects = config.validate_projects().unwrap();
        assert_eq!(projects[0].ticket_source, TicketSource::AcceptsExternal);
        config.validate_trust_routing(&projects).unwrap();

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&project_dir);
    }

    #[test]
    fn external_accepting_project_with_only_unsandboxed_profiles_fails_validation() {
        let (path, project_dir) = write_trust_routing_config(
            r#"ticket_source = "accepts-external""#,
            r#"execution_backend = "Local"
            unsandboxed = true"#,
        );

        let config = Config::load(&path).unwrap();
        let projects = config.validate_projects().unwrap();
        let err = config.validate_trust_routing(&projects).unwrap_err();
        let message = err.to_string();
        assert!(message.contains(&project_dir.display().to_string()));
        assert!(message.contains("Sandboxed"));

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&project_dir);
    }

    #[test]
    fn operator_only_project_with_only_unsandboxed_profiles_passes() {
        let (path, project_dir) = write_trust_routing_config(
            "", // default ticket_source = OperatorOnly
            r#"execution_backend = "Local"
            unsandboxed = true"#,
        );

        let config = Config::load(&path).unwrap();
        let projects = config.validate_projects().unwrap();
        assert_eq!(projects[0].ticket_source, TicketSource::OperatorOnly);
        config.validate_trust_routing(&projects).unwrap();

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir_all(&project_dir);
    }
}
