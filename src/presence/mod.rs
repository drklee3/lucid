//! Presence detection as a pluggable source list (docs/design.md, resolved decision #1).
//!
//! Any number of sources implement [`PresenceSource`]; the orchestrator composes
//! them conservatively — any source reporting "not idle" wins. `logind` is the
//! reference implementation, not the only one that will ever exist.

pub mod logind;

use async_trait::async_trait;
use std::time::Duration;

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
}
