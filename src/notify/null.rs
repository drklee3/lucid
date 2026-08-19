//! No-op sink — matches today's behavior exactly. Default when `[notifications]`
//! is absent or `backend` is unset.

use super::NotificationSink;
use crate::tracker::TrackerIssue;

pub struct NullSink;

#[async_trait::async_trait]
impl NotificationSink for NullSink {
    async fn on_awaiting_input(
        &self,
        _issue: &TrackerIssue,
        _question: &str,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn on_needs_review(
        &self,
        _issue: &TrackerIssue,
        _pr_url: Option<&str>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    async fn on_done(&self, _issue: &TrackerIssue) -> anyhow::Result<()> {
        Ok(())
    }
}
