//! Config loading — harness profiles, tracker settings, presence thresholds, and
//! the daemon's own tick/timeout knobs.

use crate::harness::HarnessProfile;
use crate::worker::CompletionMode;
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
}

#[derive(Debug, Serialize, Deserialize)]
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
    /// Where `FileTracker` persists its JSON store — required when
    /// `backend = "file"`, unused by other backends.
    #[serde(default)]
    pub file_path: Option<PathBuf>,
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
    /// Directory dispatched harnesses run in. A git worktree once worktree
    /// management exists (docs/FEATURES.md § Worker / dispatch); any directory
    /// works for now — defaults to the current directory.
    #[serde(default = "default_workdir")]
    pub workdir: PathBuf,
    /// How a successful dispatch's changes get committed — see
    /// docs/wiki/architecture/worker-completion.md. Defaults to `None`: lucid
    /// doesn't touch git, matching every behavior before this field existed.
    #[serde(default)]
    pub completion_mode: CompletionMode,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            tick_interval_secs: default_tick_interval_secs(),
            stall_timeout_secs: default_stall_timeout_secs(),
            pm_wake_interval_mins: default_pm_wake_interval_mins(),
            workdir: default_workdir(),
            completion_mode: CompletionMode::default(),
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
        Ok(config)
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
        let path = std::env::temp_dir().join(format!("lucid-config-test-{}.toml", uuid::Uuid::new_v4()));
        std::fs::write(&path, toml).unwrap();

        let config = Config::load(&path).unwrap();
        assert_eq!(config.harness_profiles.len(), 1);
        assert_eq!(config.tracker.backend, "file");
        assert_eq!(config.daemon.tick_interval_secs, 30);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_errors_instead_of_panicking() {
        let err = Config::load(Path::new("/nonexistent/lucid.toml")).unwrap_err();
        assert!(err.to_string().contains("reading config"));
    }
}
