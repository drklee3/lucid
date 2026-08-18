//! Mode-transition audit log — one JSON line per `PresenceMode` flip, appended to
//! a file sibling to the override state file (see
//! `config::default_override_path`). Distinct from [`super::override_file`]: that
//! file is current *state*, this one is an append-only *history* of what changed
//! and when, for operator trust/audit rather than runtime decisions.

use super::PresenceMode;
use chrono::{DateTime, Utc};
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct AuditLog {
    path: PathBuf,
}

impl AuditLog {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Sibling of the override state file — `presence-override` next to
    /// `presence-audit.log` under the same directory.
    #[must_use]
    pub fn default_path_from_override(override_path: &Path) -> PathBuf {
        match override_path.parent() {
            Some(parent) => parent.join("presence-audit.log"),
            None => PathBuf::from("presence-audit.log"),
        }
    }

    /// Appends a transition line if `current` differs from `previous`. No line is
    /// written when `previous` is `None` (nothing to compare against yet, i.e. the
    /// first tick) or when the mode is unchanged.
    ///
    /// # Errors
    /// Returns an error if the parent directory can't be created or the file
    /// can't be opened/written.
    pub fn record(
        &self,
        previous: Option<PresenceMode>,
        current: PresenceMode,
    ) -> anyhow::Result<()> {
        let Some((from, to)) = transition(previous, current) else {
            return Ok(());
        };
        self.append(from, to, Utc::now())
    }

    fn append(
        &self,
        from: PresenceMode,
        to: PresenceMode,
        timestamp: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        writeln!(file, "{}", format_line(from, to, timestamp))?;
        Ok(())
    }
}

/// `Some((from, to))` if `previous` exists and differs from `current`; `None`
/// otherwise (first tick, or mode unchanged).
fn transition(
    previous: Option<PresenceMode>,
    current: PresenceMode,
) -> Option<(PresenceMode, PresenceMode)> {
    let prev = previous?;
    (prev != current).then_some((prev, current))
}

fn format_line(from: PresenceMode, to: PresenceMode, timestamp: DateTime<Utc>) -> String {
    serde_json::json!({
        "timestamp": timestamp.to_rfc3339(),
        "from": mode_str(from),
        "to": mode_str(to),
    })
    .to_string()
}

fn mode_str(mode: PresenceMode) -> &'static str {
    match mode {
        PresenceMode::Active => "active",
        PresenceMode::Autonomous => "autonomous",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_previous_mode_means_no_transition() {
        assert_eq!(transition(None, PresenceMode::Active), None);
        assert_eq!(transition(None, PresenceMode::Autonomous), None);
    }

    #[test]
    fn unchanged_mode_means_no_transition() {
        assert_eq!(
            transition(Some(PresenceMode::Active), PresenceMode::Active),
            None
        );
        assert_eq!(
            transition(Some(PresenceMode::Autonomous), PresenceMode::Autonomous),
            None
        );
    }

    #[test]
    fn changed_mode_is_a_transition() {
        assert_eq!(
            transition(Some(PresenceMode::Active), PresenceMode::Autonomous),
            Some((PresenceMode::Active, PresenceMode::Autonomous))
        );
        assert_eq!(
            transition(Some(PresenceMode::Autonomous), PresenceMode::Active),
            Some((PresenceMode::Autonomous, PresenceMode::Active))
        );
    }

    #[test]
    fn record_appends_one_json_line_per_transition() {
        let path = std::env::temp_dir().join(format!("lucid-audit-test-{}", uuid::Uuid::new_v4()));
        let log = AuditLog::new(&path);

        log.record(None, PresenceMode::Active).unwrap();
        assert!(
            !path.exists(),
            "first tick has no previous mode, nothing to write"
        );

        log.record(Some(PresenceMode::Active), PresenceMode::Active)
            .unwrap();
        assert!(!path.exists(), "unchanged mode writes nothing");

        log.record(Some(PresenceMode::Active), PresenceMode::Autonomous)
            .unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 1);
        let entry: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(entry["from"], "active");
        assert_eq!(entry["to"], "autonomous");
        assert!(entry["timestamp"].is_string());

        log.record(Some(PresenceMode::Autonomous), PresenceMode::Active)
            .unwrap();
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(contents.lines().count(), 2);

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn default_path_from_override_is_a_sibling_file() {
        let override_path = PathBuf::from("/tmp/lucid/presence-override");
        assert_eq!(
            AuditLog::default_path_from_override(&override_path),
            PathBuf::from("/tmp/lucid/presence-audit.log")
        );
    }
}
