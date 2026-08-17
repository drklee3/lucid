//! Config loading — harness profiles, tracker settings, presence thresholds.
//!
//! Shape only; `load()` is not implemented yet.

use crate::harness::HarnessProfile;
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct Config {
    pub harness_profiles: Vec<HarnessProfile>,
    pub tracker: TrackerConfig,
    pub presence: PresenceConfig,
}

#[derive(Debug, Deserialize)]
pub struct TrackerConfig {
    pub backend: String,
    pub api_key_env: String,
}

#[derive(Debug, Deserialize)]
pub struct PresenceConfig {
    /// Minutes of sustained idle before flipping to autonomous mode.
    pub idle_threshold_minutes: u32,
    /// Max proposals a PM wake cycle may file.
    pub proposal_cap_per_wake: u32,
}

impl Config {
    pub fn load(_path: &Path) -> anyhow::Result<Self> {
        todo!("parse TOML config file into Config")
    }
}
