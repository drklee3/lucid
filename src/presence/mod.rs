//! Presence detection as a pluggable source list (see docs/wiki/architecture/presence-detection.md).
//!
//! Any number of sources implement [`PresenceSource`]; the orchestrator composes
//! them conservatively — any source reporting "not idle" wins. `logind` is the
//! reference implementation, not the only one that will ever exist.

pub mod audit_log;
pub mod logind;
pub mod override_file;

use async_trait::async_trait;
use override_file::{OverrideFile, OverrideMode};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresenceMode {
    Active,
    Autonomous,
}

/// Async: real sources (`logind`) read their idle state over D-Bus, not from a
/// pre-warmed local cache — see docs/wiki (presence detection) for why this trait
/// isn't sync-with-a-background-cache instead.
#[async_trait]
pub trait PresenceSource: Send + Sync {
    /// Human-readable name for this source, shown in `lucid presence status`.
    fn name(&self) -> &str;

    async fn is_idle(&self) -> bool;

    /// How long the source believes the user has been idle, if it can say.
    async fn idle_since(&self) -> Option<Duration>;
}

/// Composes multiple sources conservatively: idle only if every source agrees.
pub struct PresenceSourceList {
    sources: Vec<Box<dyn PresenceSource>>,
}

impl PresenceSourceList {
    #[must_use]
    pub fn new(sources: Vec<Box<dyn PresenceSource>>) -> Self {
        Self { sources }
    }

    pub async fn is_idle(&self) -> bool {
        for s in &self.sources {
            if !s.is_idle().await {
                return false;
            }
        }
        true
    }

    pub async fn readings(&self) -> Vec<(&str, bool, Option<Duration>)> {
        let mut out = Vec::with_capacity(self.sources.len());
        for s in &self.sources {
            out.push((s.name(), s.is_idle().await, s.idle_since().await));
        }
        out
    }

    /// The shortest `idle_since` among sources that report idle at all — the
    /// debounce check needs the *least*-idle source's duration, since composition
    /// already requires every source to agree before this matters. `None` if any
    /// idle source can't say how long (treated as not-yet-sustained), or if the
    /// list is empty.
    async fn min_idle_duration(&self) -> Option<Duration> {
        let mut min = None;
        for s in &self.sources {
            let since = s.idle_since().await?;
            min = Some(min.map_or(since, |m: Duration| m.min(since)));
        }
        min
    }
}

/// Resolves the daemon's actual operating mode for this tick: the override file
/// takes priority and skips the debounce entirely (`Active`/`Autonomous` are
/// immediate, deliberate, human decisions); with no override, every configured
/// source must report idle *and* have done so for at least `idle_threshold` before
/// the mode flips to autonomous — a source that's idle but can't say for how long,
/// or an empty source list, is treated conservatively as not-yet-sustained (stays
/// `Active`). See docs/wiki/architecture/presence-detection.md.
///
/// # Errors
/// Returns an error if the override file exists but can't be read or parsed.
pub async fn resolve(
    sources: &PresenceSourceList,
    override_file: &OverrideFile,
    idle_threshold: Duration,
) -> anyhow::Result<PresenceMode> {
    match override_file.read()? {
        OverrideMode::Active => return Ok(PresenceMode::Active),
        OverrideMode::Autonomous => return Ok(PresenceMode::Autonomous),
        OverrideMode::Auto => {}
    }

    if !sources.is_idle().await {
        return Ok(PresenceMode::Active);
    }
    let sustained = sources
        .min_idle_duration()
        .await
        .is_some_and(|d| d >= idle_threshold);
    Ok(if sustained {
        PresenceMode::Autonomous
    } else {
        PresenceMode::Active
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeSource {
        idle: bool,
        since: Option<Duration>,
    }

    #[async_trait]
    impl PresenceSource for FakeSource {
        fn name(&self) -> &'static str {
            "fake"
        }
        async fn is_idle(&self) -> bool {
            self.idle
        }
        async fn idle_since(&self) -> Option<Duration> {
            self.since
        }
    }

    fn override_file() -> OverrideFile {
        OverrideFile::new(
            std::env::temp_dir().join(format!("lucid-resolve-test-{}", uuid::Uuid::new_v4())),
        )
    }

    #[tokio::test]
    async fn override_active_wins_even_if_sources_are_idle() {
        let sources = PresenceSourceList::new(vec![Box::new(FakeSource {
            idle: true,
            since: Some(Duration::from_secs(9999)),
        })]);
        let file = override_file();
        file.write(OverrideMode::Active).unwrap();
        let mode = resolve(&sources, &file, Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(mode, PresenceMode::Active);
    }

    #[tokio::test]
    async fn idle_but_not_sustained_stays_active() {
        let sources = PresenceSourceList::new(vec![Box::new(FakeSource {
            idle: true,
            since: Some(Duration::from_secs(5)),
        })]);
        let file = override_file();
        let mode = resolve(&sources, &file, Duration::from_secs(1200))
            .await
            .unwrap();
        assert_eq!(mode, PresenceMode::Active);
    }

    #[tokio::test]
    async fn idle_and_sustained_past_threshold_goes_autonomous() {
        let sources = PresenceSourceList::new(vec![Box::new(FakeSource {
            idle: true,
            since: Some(Duration::from_secs(1800)),
        })]);
        let file = override_file();
        let mode = resolve(&sources, &file, Duration::from_secs(1200))
            .await
            .unwrap();
        assert_eq!(mode, PresenceMode::Autonomous);
    }

    #[tokio::test]
    async fn empty_source_list_defaults_to_active() {
        let sources = PresenceSourceList::new(vec![]);
        let file = override_file();
        let mode = resolve(&sources, &file, Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(mode, PresenceMode::Active);
    }
}
