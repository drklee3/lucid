//! Reference [`PresenceSource`] implementation: `systemd-logind` over D-Bus.
//!
//! Watches `org.freedesktop.login1.Session` for `Lock`/`Unlock` signals and reads
//! `IdleHint`/`IdleSinceHint`. Correct on a real Linux desktop.
//!
//! **Known gap, see docs/wiki/architecture/presence-detection.md:** on WSL2 this
//! machine's `IdleHint` is stuck at `true` — there's no real "seat" with physical
//! input devices behind a WSL2 pty session, so nothing ever resets it. That doesn't
//! block building this source (it's still the correct reference implementation for
//! a normal Linux desktop), it just means this source alone is insufficient for
//! presence-gated autonomy *on this machine* until a second source is added to the
//! list. Real D-Bus wiring (a `zbus`-backed connection, signal subscription) is not
//! implemented yet — this is the shape, not the behavior. On the `PresenceSource`
//! trait itself being `async` (needed for this D-Bus read), see
//! docs/wiki/architecture/presence-detection.md.

use super::PresenceSource;
use async_trait::async_trait;
use std::time::Duration;

pub struct LogindSource {
    // Real implementation holds a zbus::Connection + the session object path.
    // Left unconstructed here — see module doc.
}

impl LogindSource {
    /// # Errors
    /// The real implementation will fail if the system D-Bus can't be reached or
    /// no session can be resolved for the current process; the stub never errors.
    pub fn new() -> anyhow::Result<Self> {
        // TODO: connect to the system bus, resolve the current session via
        // org.freedesktop.login1.Manager.GetSessionByPID, subscribe to
        // Lock/Unlock signals.
        Ok(Self {})
    }
}

#[async_trait]
impl PresenceSource for LogindSource {
    fn name(&self) -> &'static str {
        "logind"
    }

    async fn is_idle(&self) -> bool {
        todo!("read IdleHint over D-Bus")
    }

    async fn idle_since(&self) -> Option<Duration> {
        todo!("read IdleSinceHint over D-Bus")
    }
}
