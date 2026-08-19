//! Script-backed `NotificationSink` — the extensibility pilot (see
//! docs/wiki/architecture/extensibility-primitives.md § Pilot). One-shot
//! subprocess per event: a JSON payload on stdin, fire-and-forget — stdout and
//! exit status are not treated as failures, only spawn errors and timeouts are.

use super::NotificationSink;
use crate::tracker::TrackerIssue;
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub struct ScriptSink {
    dir: PathBuf,
    timeout: Duration,
}

impl ScriptSink {
    #[must_use]
    pub fn new(dir: PathBuf, timeout: Duration) -> Self {
        Self { dir, timeout }
    }

    /// Runs `event`'s script if one exists and is executable at `dir/<event>`;
    /// a silent no-op otherwise (not every event needs a script wired up).
    /// Errors are returned for the caller to log, never meant to be propagated
    /// as a reason to fail the caller's real work.
    async fn fire(&self, event: &str, payload: &impl Serialize) -> anyhow::Result<()> {
        let script_path = self.dir.join(event);
        if !is_executable(&script_path) {
            return Ok(());
        }

        let body = serde_json::to_vec(payload)?;

        let mut child = Command::new(&script_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| anyhow::anyhow!("spawning {}: {e}", script_path.display()))?;

        if let Some(mut stdin) = child.stdin.take() {
            // A script that exits without reading stdin makes this write fail;
            // that's not itself an error here, the process may still run to
            // completion having ignored its input.
            let _ = stdin.write_all(&body).await;
            drop(stdin); // closes stdin (EOF)
        }

        tokio::time::timeout(self.timeout, child.wait())
            .await
            .map_err(|_| {
                anyhow::anyhow!(
                    "notify script {} timed out after {:?}",
                    script_path.display(),
                    self.timeout
                )
            })?
            .map_err(|e| anyhow::anyhow!("waiting on {}: {e}", script_path.display()))?;

        Ok(())
    }
}

#[cfg(unix)]
fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &std::path::Path) -> bool {
    path.is_file()
}

const PROTOCOL: &str = "lucid.plugin/1";

/// One payload shape reused across all three events, irrelevant fields omitted.
/// `"protocol"` pins extensibility-primitives.md's wire-version string, included
/// now even though the version-negotiation machinery around it isn't built yet,
/// since adding it later would be a breaking payload change.
///
/// `issue.decision_state` reflects whatever it was when the tracker object was
/// fetched — typically *before* this event's own state transition, since the
/// sink fires after `set_decision_state` succeeds. `event` is the source of
/// truth for what happened, not `issue.decision_state`.
#[derive(Serialize)]
struct EventPayload<'a> {
    protocol: &'static str,
    event: &'static str,
    issue: &'a TrackerIssue,
    #[serde(skip_serializing_if = "Option::is_none")]
    pr_url: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    question: Option<&'a str>,
}

#[async_trait::async_trait]
impl NotificationSink for ScriptSink {
    async fn on_awaiting_input(&self, issue: &TrackerIssue, question: &str) -> anyhow::Result<()> {
        self.fire(
            "on_awaiting_input",
            &EventPayload {
                protocol: PROTOCOL,
                event: "on_awaiting_input",
                issue,
                pr_url: None,
                question: Some(question),
            },
        )
        .await
    }

    async fn on_needs_review(
        &self,
        issue: &TrackerIssue,
        pr_url: Option<&str>,
    ) -> anyhow::Result<()> {
        self.fire(
            "on_needs_review",
            &EventPayload {
                protocol: PROTOCOL,
                event: "on_needs_review",
                issue,
                pr_url,
                question: None,
            },
        )
        .await
    }

    async fn on_done(&self, issue: &TrackerIssue) -> anyhow::Result<()> {
        self.fire(
            "on_done",
            &EventPayload {
                protocol: PROTOCOL,
                event: "on_done",
                issue,
                pr_url: None,
                question: None,
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::{DecisionState, ReviewMode};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn sample_issue() -> TrackerIssue {
        TrackerIssue {
            id: "issue-1".to_string(),
            title: "Test issue".to_string(),
            description: None,
            decision_state: Some(DecisionState::Approved),
            review: ReviewMode::Auto,
            identifier: None,
        }
    }

    fn write_script(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, body).unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
        path
    }

    #[tokio::test]
    async fn missing_script_is_a_silent_no_op() {
        let dir = tempfile_dir();
        let sink = ScriptSink::new(dir.clone(), Duration::from_secs(5));
        sink.on_done(&sample_issue()).await.unwrap();
        assert!(!dir.join("on_done").exists());
    }

    #[tokio::test]
    async fn non_executable_script_is_a_silent_no_op() {
        let dir = tempfile_dir();
        let path = dir.join("on_done");
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        // No chmod +x — file exists but isn't executable.
        let sink = ScriptSink::new(dir, Duration::from_secs(5));
        sink.on_done(&sample_issue()).await.unwrap();
    }

    #[tokio::test]
    async fn successful_script_receives_the_documented_payload() {
        let dir = tempfile_dir();
        let out_path = dir.join("captured.json");
        write_script(
            &dir,
            "on_needs_review",
            &format!("#!/bin/sh\ncat > {}\n", out_path.display()),
        );
        let sink = ScriptSink::new(dir, Duration::from_secs(5));
        sink.on_needs_review(&sample_issue(), Some("https://github.com/o/r/pull/1"))
            .await
            .unwrap();

        let captured: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&out_path).unwrap()).unwrap();
        assert_eq!(captured["protocol"], "lucid.plugin/1");
        assert_eq!(captured["event"], "on_needs_review");
        assert_eq!(captured["issue"]["id"], "issue-1");
        assert_eq!(captured["pr_url"], "https://github.com/o/r/pull/1");
        assert!(captured.get("question").is_none());
    }

    #[tokio::test]
    async fn nonzero_exit_is_not_an_error() {
        let dir = tempfile_dir();
        write_script(&dir, "on_done", "#!/bin/sh\nexit 1\n");
        let sink = ScriptSink::new(dir, Duration::from_secs(5));
        sink.on_done(&sample_issue()).await.unwrap();
    }

    #[tokio::test]
    async fn script_ignoring_stdin_does_not_error() {
        let dir = tempfile_dir();
        write_script(&dir, "on_done", "#!/bin/sh\nexit 0\n");
        let sink = ScriptSink::new(dir, Duration::from_secs(5));
        sink.on_done(&sample_issue()).await.unwrap();
    }

    #[tokio::test]
    async fn slow_script_times_out() {
        let dir = tempfile_dir();
        write_script(&dir, "on_done", "#!/bin/sh\nsleep 30\n");
        let sink = ScriptSink::new(dir, Duration::from_millis(200));
        let err = sink.on_done(&sample_issue()).await.unwrap_err();
        assert!(err.to_string().contains("timed out"));
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lucid-notify-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
