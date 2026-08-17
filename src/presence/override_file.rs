//! Explicit override layer (docs/FEATURES.md § Presence): a state file that always
//! wins over automatic sources, no debounce. Deliberately *not* a [`PresenceSource`]
//! implementation — override isn't a composable idle signal, it's a short-circuit
//! that pre-empts consulting the other sources at all (`Active` forces attended
//! mode even if every source says idle; `Autonomous` forces it on even if nothing
//! looks idle). See [`resolve`].
//!
//! [`PresenceSource`]: super::PresenceSource

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverrideMode {
    /// Force attended mode — never dispatch autonomously, regardless of sources.
    Active,
    /// Force autonomous mode now, regardless of sources.
    Autonomous,
    /// No override; defer to the automatic source composition.
    Auto,
}

impl OverrideMode {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            OverrideMode::Active => "active",
            OverrideMode::Autonomous => "autonomous",
            OverrideMode::Auto => "auto",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim() {
            "active" => Some(OverrideMode::Active),
            "autonomous" => Some(OverrideMode::Autonomous),
            "auto" => Some(OverrideMode::Auto),
            _ => None,
        }
    }
}

pub struct OverrideFile {
    path: PathBuf,
}

impl OverrideFile {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Missing file reads as `Auto` — no override set is the common case, not an
    /// error.
    ///
    /// # Errors
    /// Returns an error if the file exists but can't be read, or its content
    /// doesn't parse as one of `active`/`autonomous`/`auto`.
    pub fn read(&self) -> anyhow::Result<OverrideMode> {
        if !self.path.exists() {
            return Ok(OverrideMode::Auto);
        }
        let data = std::fs::read_to_string(&self.path)?;
        OverrideMode::parse(&data)
            .ok_or_else(|| anyhow::anyhow!("unrecognized override mode in {}: {data:?}", self.path.display()))
    }

    /// # Errors
    /// Returns an error if the parent directory can't be created or the file
    /// can't be written.
    pub fn write(&self, mode: OverrideMode) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, mode.as_str())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_path() -> PathBuf {
        std::env::temp_dir().join(format!("lucid-override-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn missing_file_reads_as_auto() {
        let file = OverrideFile::new(scratch_path());
        assert_eq!(file.read().unwrap(), OverrideMode::Auto);
    }

    #[test]
    fn write_then_read_round_trips() {
        let file = OverrideFile::new(scratch_path());
        file.write(OverrideMode::Autonomous).unwrap();
        assert_eq!(file.read().unwrap(), OverrideMode::Autonomous);
        let _ = std::fs::remove_file(file.path());
    }

    #[test]
    fn garbage_content_errors_rather_than_silently_defaulting() {
        let path = scratch_path();
        std::fs::write(&path, "not-a-real-mode").unwrap();
        let file = OverrideFile::new(&path);
        assert!(file.read().is_err());
        let _ = std::fs::remove_file(&path);
    }
}
