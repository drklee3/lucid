//! PM gap-detection wake cycle: investigates the repo against a stated goal and
//! files concrete gap-flag proposals — not open-ended ideation (see
//! docs/wiki/architecture/pm-scope.md). Read-mostly and low-stakes by design: PM
//! dispatches under a restricted `--allowedTools` list, never the Worker's full
//! `auto` permission mode (see docs/FEATURES.md § PM / gap-detection).

use crate::config::ObservabilityConfig;
use crate::harness::{self, DispatchRequest, HarnessProfile, TelemetryConfig};
use crate::tracker::{EffortEstimate, Proposal, ReviewMode, TrackerAdapter};
use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

/// Enumerated rather than a blanket read-only mode: PM's actual needs are "look at
/// history and files," not "everything Manual mode happens to auto-approve."
const PM_CLAUDE_ARGS: &[&str] = &[
    "--allowedTools",
    "Read,Grep,Glob,Bash(git log *),Bash(git status *),Bash(git diff *),Bash(ls *)",
];

#[derive(Debug, Deserialize)]
struct RawProposal {
    title: String,
    summary: String,
    #[serde(default)]
    why_now: Vec<String>,
    effort_estimate: String,
    risk_note: String,
    task_type: String,
    #[serde(default)]
    target_paths: Vec<String>,
    #[serde(default)]
    acceptance_criteria: Vec<String>,
}

fn parse_effort(s: &str) -> EffortEstimate {
    match s.trim().to_lowercase().as_str() {
        "s" | "small" => EffortEstimate::Small,
        "l" | "large" => EffortEstimate::Large,
        _ => EffortEstimate::Medium,
    }
}

fn build_prompt(goal: &str) -> String {
    format!(
        "You are a PM agent investigating this repository against a stated goal.\n\
         Goal: {goal}\n\n\
         Look at recent git history, the current file tree, and any wiki/ROADMAP \
         content. Find at most a few concrete gaps: something the goal implies that \
         nothing in the repo currently tracks or addresses. Do not propose \
         speculative or vague work — only concrete, scoped gaps. If there are no \
         genuine gaps, that's a valid outcome.\n\n\
         Respond with ONLY a JSON array (no prose, no markdown fences) of objects, \
         each with exactly these fields: title (string), summary (one sentence), \
         why_now (array of 2-3 short strings), effort_estimate (\"S\", \"M\", or \
         \"L\"), risk_note (string), task_type (string), target_paths (array of \
         strings), acceptance_criteria (array of strings). Respond with [] if \
         there are no gaps."
    )
}

/// Extracts a JSON array from the harness's result text — tolerates the model
/// wrapping it in prose or a markdown fence despite the prompt asking for neither,
/// by taking the outermost `[`...`]` span rather than requiring the whole string
/// to be exactly the array.
fn extract_json_array(text: &str) -> Option<&str> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    (end >= start).then(|| &text[start..=end])
}

#[derive(Debug)]
pub struct WakeOutcome {
    /// Every proposal the harness returned, capped at `cap`, before dedup.
    pub proposed: Vec<Proposal>,
    /// Issue ids actually created — empty when `dry_run` is set.
    pub filed: Vec<String>,
    /// Titles skipped because `query_similar` already found something like them —
    /// the death-loop-prevention check (docs/wiki/architecture/dedup-death-loop.md).
    pub skipped_similar: Vec<String>,
}

/// Runs one PM investigation and files (or, if `dry_run`, just reports) the
/// resulting proposals.
///
/// # Errors
/// Returns an error if the harness dispatch itself fails or times out, or its
/// output can't be parsed as the expected JSON array — a PM wake that can't parse
/// its own output is a hard failure, not silently zero proposals.
#[allow(clippy::too_many_arguments)]
pub async fn wake(
    tracker: &dyn TrackerAdapter,
    profiles: &[HarnessProfile],
    observability: &ObservabilityConfig,
    workdir: &Path,
    goal: &str,
    cap: u32,
    timeout: Duration,
    dry_run: bool,
) -> anyhow::Result<WakeOutcome> {
    let telemetry = TelemetryConfig {
        otlp_endpoint: observability.otlp_endpoint.clone(),
        log_prompts: observability.log_prompts,
    };
    let prompt = build_prompt(goal);

    let outcome = harness::dispatch_with_fallback(DispatchRequest {
        profiles,
        prompt: &prompt,
        ticket_id: "pm-wake",
        telemetry: &telemetry,
        workdir,
        timeout,
        claude_extra_args: PM_CLAUDE_ARGS,
    })
    .await?;

    let result_text = outcome
        .result_text
        .ok_or_else(|| anyhow::anyhow!("PM wake dispatch produced no result text"))?;
    let json = extract_json_array(&result_text).ok_or_else(|| {
        anyhow::anyhow!("PM wake result didn't contain a JSON array: {result_text}")
    })?;
    let raw: Vec<RawProposal> = serde_json::from_str(json)
        .map_err(|e| anyhow::anyhow!("PM wake result wasn't valid JSON: {e}\n{json}"))?;

    let mut proposed = Vec::new();
    let mut filed = Vec::new();
    let mut skipped_similar = Vec::new();

    for r in raw.into_iter().take(cap as usize) {
        let proposal = Proposal {
            title: r.title,
            summary: r.summary,
            why_now: r.why_now,
            effort_estimate: parse_effort(&r.effort_estimate),
            risk_note: r.risk_note,
            task_type: r.task_type,
            target_paths: r.target_paths,
            acceptance_criteria: r.acceptance_criteria,
            research_ref: None,
            // PM-filed proposals don't pick a review mode yet — not exposed in
            // the wake prompt's JSON schema this pass (see docs/FEATURES.md §
            // PM / gap-detection). A human can retag the issue after filing.
            review: ReviewMode::Auto,
            verify_cmd: None,
        };

        if !tracker.query_similar(&proposal.title).await?.is_empty() {
            skipped_similar.push(proposal.title.clone());
            proposed.push(proposal);
            continue;
        }

        if !dry_run {
            filed.push(tracker.create_proposal(&proposal).await?);
        }
        proposed.push(proposal);
    }

    Ok(WakeOutcome {
        proposed,
        filed,
        skipped_similar,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{AuthMode, HarnessKind};
    use crate::tracker::file::FileTracker;

    #[test]
    fn extract_json_array_ignores_surrounding_prose() {
        let text = "Sure, here you go:\n```json\n[{\"a\":1}]\n```\nHope that helps!";
        assert_eq!(extract_json_array(text), Some(r#"[{"a":1}]"#));
    }

    #[test]
    fn extract_json_array_handles_empty_array() {
        assert_eq!(extract_json_array("[]"), Some("[]"));
    }

    #[test]
    fn extract_json_array_returns_none_without_brackets() {
        assert_eq!(extract_json_array("no array here"), None);
    }

    #[test]
    fn parse_effort_is_case_and_word_form_tolerant() {
        assert!(matches!(parse_effort("S"), EffortEstimate::Small));
        assert!(matches!(parse_effort("small"), EffortEstimate::Small));
        assert!(matches!(parse_effort("L"), EffortEstimate::Large));
        assert!(matches!(parse_effort("weird"), EffortEstimate::Medium));
    }

    fn observability() -> ObservabilityConfig {
        ObservabilityConfig {
            otlp_endpoint: "http://localhost:4317".to_string(),
            log_prompts: false,
            trace_ui_base_url: "http://localhost:6006".to_string(),
            trace_ui_project_id: None,
        }
    }

    /// `sh -c '<script>'` that ignores every argument it's called with (the
    /// telemetry/dispatch flags `apply_dispatch_flags` appends, since the script
    /// body never references `$1`/etc.) and always emits one canned `result`
    /// stream-json event — a stand-in for a real `claude -p` harness for testing
    /// `wake`'s parse/dedup/file logic without spawning the real binary. Inlined
    /// via `-c` rather than written to a script file and exec'd: a
    /// write-then-immediately-exec on a fresh file is a real, observed race
    /// (`ETXTBSY`/"Text file busy") under concurrent test execution — see the same
    /// pattern already used by `harness::tests::a_hanging_process_times_out_and_is_killed`.
    /// Safe to single-quote as-is: `serde_json::to_string` never emits a literal
    /// `'`.
    fn fake_pm_harness(result_json: &str) -> HarnessProfile {
        let event = serde_json::json!({
            "type": "result",
            "subtype": "success",
            "is_error": false,
            "result": result_json,
        })
        .to_string();
        HarnessProfile {
            name: "fake-pm".to_string(),
            kind: HarnessKind::ClaudeCode,
            cmd: "sh".to_string(),
            args: vec!["-c".to_string(), format!("echo '{event}'")],
            auth_mode: AuthMode::Subscription,
            priority: 1,
        }
    }

    #[tokio::test]
    async fn wake_files_a_new_proposal_then_dedups_it_on_the_next_wake() {
        let gap = serde_json::json!([{
            "title": "Add rate limiting to the tracker adapter",
            "summary": "no backoff on Linear 429s",
            "why_now": ["seen in practitioner-reality survey"],
            "effort_estimate": "S",
            "risk_note": "low",
            "task_type": "feature",
            "target_paths": ["src/tracker/linear.rs"],
            "acceptance_criteria": ["retries with backoff on 429"],
        }])
        .to_string();
        let profile = fake_pm_harness(&gap);
        let tracker_path =
            std::env::temp_dir().join(format!("lucid-pm-test-{}.json", uuid::Uuid::new_v4()));
        let tracker = FileTracker::open(&tracker_path).unwrap();

        let first = wake(
            &tracker,
            std::slice::from_ref(&profile),
            &observability(),
            &std::env::temp_dir(),
            "harden the tracker adapter",
            5,
            Duration::from_secs(5),
            false,
        )
        .await
        .unwrap();
        assert_eq!(first.filed.len(), 1);
        assert!(first.skipped_similar.is_empty());

        let second = wake(
            &tracker,
            std::slice::from_ref(&profile),
            &observability(),
            &std::env::temp_dir(),
            "harden the tracker adapter",
            5,
            Duration::from_secs(5),
            false,
        )
        .await
        .unwrap();
        assert!(second.filed.is_empty());
        assert_eq!(second.skipped_similar.len(), 1);

        let _ = std::fs::remove_file(&tracker_path);
    }

    #[tokio::test]
    async fn wake_dry_run_reports_without_filing() {
        let gap = serde_json::json!([{
            "title": "Some gap",
            "summary": "s",
            "why_now": [],
            "effort_estimate": "M",
            "risk_note": "r",
            "task_type": "t",
            "target_paths": [],
            "acceptance_criteria": [],
        }])
        .to_string();
        let profile = fake_pm_harness(&gap);
        let tracker_path =
            std::env::temp_dir().join(format!("lucid-pm-test-{}.json", uuid::Uuid::new_v4()));
        let tracker = FileTracker::open(&tracker_path).unwrap();

        let outcome = wake(
            &tracker,
            &[profile],
            &observability(),
            &std::env::temp_dir(),
            "goal",
            5,
            Duration::from_secs(5),
            true,
        )
        .await
        .unwrap();
        assert_eq!(outcome.proposed.len(), 1);
        assert!(outcome.filed.is_empty());

        let _ = std::fs::remove_file(&tracker_path);
    }

    #[tokio::test]
    async fn wake_respects_the_proposal_cap() {
        let gaps = serde_json::json!([
            {"title": "A", "summary": "s", "why_now": [], "effort_estimate": "S", "risk_note": "r", "task_type": "t", "target_paths": [], "acceptance_criteria": []},
            {"title": "B", "summary": "s", "why_now": [], "effort_estimate": "S", "risk_note": "r", "task_type": "t", "target_paths": [], "acceptance_criteria": []},
            {"title": "C", "summary": "s", "why_now": [], "effort_estimate": "S", "risk_note": "r", "task_type": "t", "target_paths": [], "acceptance_criteria": []},
        ])
        .to_string();
        let profile = fake_pm_harness(&gaps);
        let tracker_path =
            std::env::temp_dir().join(format!("lucid-pm-test-{}.json", uuid::Uuid::new_v4()));
        let tracker = FileTracker::open(&tracker_path).unwrap();

        let outcome = wake(
            &tracker,
            &[profile],
            &observability(),
            &std::env::temp_dir(),
            "goal",
            2,
            Duration::from_secs(5),
            false,
        )
        .await
        .unwrap();
        assert_eq!(outcome.proposed.len(), 2);

        let _ = std::fs::remove_file(&tracker_path);
    }
}
