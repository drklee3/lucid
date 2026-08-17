//! Presence detection as a pluggable source list (docs/design.md, resolved decision #1).
//!
//! Any number of sources implement [`PresenceSource`]; the orchestrator composes
//! them conservatively — any source reporting "not idle" wins. `logind` is the
//! reference implementation, not the only one that will ever exist.

pub mod logind;

use std::time::Duration;

pub trait PresenceSource: Send + Sync {
    /// Human-readable name for this source, shown in `lucid presence status`.
    fn name(&self) -> &str;

    fn is_idle(&self) -> bool;

    /// How long the source believes the user has been idle, if it can say.
    fn idle_since(&self) -> Option<Duration>;
}

/// Composes multiple sources conservatively: idle only if every source agrees.
pub struct PresenceSourceList {
    sources: Vec<Box<dyn PresenceSource>>,
}

impl PresenceSourceList {
    pub fn new(sources: Vec<Box<dyn PresenceSource>>) -> Self {
        Self { sources }
    }

    pub fn is_idle(&self) -> bool {
        self.sources.iter().all(|s| s.is_idle())
    }

    pub fn readings(&self) -> Vec<(&str, bool, Option<Duration>)> {
        self.sources
            .iter()
            .map(|s| (s.name(), s.is_idle(), s.idle_since()))
            .collect()
    }
}
