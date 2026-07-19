use std::path::{Path, PathBuf};

use chrono::Utc;

use crate::error::Error;
use crate::resources::RunSummary;

/// Produce a changelog-safe copy: drop from/to/output bodies and truncate diff,
/// so the log records that a resource changed without persisting secret bodies.
pub fn redact_for_changelog(summaries: &[RunSummary]) -> Vec<RunSummary> {
    // Policy: the changelog never persists payload bodies (from/to/output) for ANY resource -
    // it records that a resource changed plus a short truncated diff, to keep the log compact
    // and avoid inadvertently persisting secrets.
    summaries
        .iter()
        .map(|s| {
            let resources = s
                .resources
                .iter()
                .map(|r| {
                    let mut r = r.clone();
                    r.from = None;
                    r.to = None;
                    r.output = None;
                    // Structured changes carry the same bodies; drop their
                    // from/to too, keeping only field/action.
                    for c in &mut r.changes {
                        c.from = None;
                        c.to = None;
                    }
                    if let Some(d) = &r.diff
                        && d.len() > 200
                    {
                        let mut end = 200;
                        while end > 0 && !d.is_char_boundary(end) {
                            end -= 1;
                        }
                        r.diff = Some(format!("{}...", &d[..end]));
                    }
                    r
                })
                .collect();
            RunSummary {
                host: s.host.clone(),
                resources,
                summary: s.summary.clone(),
            }
        })
        .collect()
}

/// Write a redacted JSON run-report for a single serve-mode convergence cycle.
///
/// The file is written to `report_dir/<timestamp>-serve.json`. The caller
/// provides a filesystem-safe timestamp (colons replaced with hyphens,
/// e.g. `%Y-%m-%dT%H-%M-%S`) so the filename is valid on all platforms.
/// Redaction reuses `redact_for_changelog`: from/to/output are stripped and
/// long diffs are truncated, matching apply-log policy.
pub fn write_serve_report(
    report_dir: &Path,
    summary: &RunSummary,
    source: &str,
    timestamp: &str,
) -> Result<PathBuf, Error> {
    std::fs::create_dir_all(report_dir)
        .map_err(|e| Error::Other(format!("failed to create report dir: {e}")))?;

    let redacted = redact_for_changelog(std::slice::from_ref(summary));
    let redacted_summary = &redacted[0];

    let report = serde_json::json!({
        "timestamp": timestamp,
        "source": source,
        "summary": redacted_summary,
    });
    let json = serde_json::to_string_pretty(&report)
        .map_err(|e| Error::Other(format!("failed to serialize serve report: {e}")))?;

    let path = report_dir.join(format!("{timestamp}-serve.json"));
    std::fs::write(&path, json)
        .map_err(|e| Error::Other(format!("failed to write serve report: {e}")))?;

    Ok(path)
}

pub fn write_log(base_dir: &Path, summaries: &[RunSummary]) -> Result<(), Error> {
    let log_dir = base_dir.join(".verg").join("logs");
    std::fs::create_dir_all(&log_dir)
        .map_err(|e| Error::Other(format!("failed to create log dir: {e}")))?;

    // Sub-second resolution so two apply runs finishing in the same second do
    // not produce the same filename and silently overwrite each other's log.
    let now = Utc::now();
    let timestamp = format!(
        "{}-{:09}",
        now.format("%Y-%m-%dT%H-%M-%S"),
        now.timestamp_subsec_nanos()
    );
    let filename = format!("{timestamp}-apply.json");
    let path = log_dir.join(filename);

    let json = serde_json::to_string_pretty(&redact_for_changelog(summaries))
        .map_err(|e| Error::Other(format!("failed to serialize log: {e}")))?;
    std::fs::write(&path, json).map_err(|e| Error::Other(format!("failed to write log: {e}")))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{ResourceResult, ResourceStatus, RunSummary};
    use tempfile::TempDir;

    #[test]
    fn changelog_drops_bulk_payloads() {
        let summaries = vec![RunSummary::from_results(
            "web1",
            vec![ResourceResult {
                resource_type: "file".into(),
                name: "secret".into(),
                status: ResourceStatus::Changed,
                diff: Some("x".repeat(1000)),
                from: Some("old".into()),
                to: Some("new secret body".into()),
                error: None,
                output: Some("captured".into()),
                changes: Vec::new(),
            }],
        )];
        let red = super::redact_for_changelog(&summaries);
        let r = &red[0].resources[0];
        assert!(r.from.is_none() && r.to.is_none() && r.output.is_none());
        assert!(
            r.diff.as_ref().unwrap().len() <= 210,
            "diff should be truncated"
        );
        assert_eq!(r.status, ResourceStatus::Changed);
    }

    #[test]
    fn write_serve_report_creates_redacted_json() {
        let dir = TempDir::new().unwrap();
        let summary = RunSummary::from_results(
            "host1",
            vec![ResourceResult {
                resource_type: "file".into(),
                name: "/etc/motd".into(),
                status: ResourceStatus::Changed,
                diff: Some("updated".into()),
                from: Some("old content".into()),
                to: Some("new content".into()),
                error: None,
                output: Some("captured output".into()),
                changes: Vec::new(),
            }],
        );

        let path = write_serve_report(
            dir.path(),
            &summary,
            "file:///tmp/b.toml",
            "2026-06-21T10-30-00",
        )
        .unwrap();

        assert!(path.exists(), "report file should exist");
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with("2026-06-21T10-30-00-serve.json"),
            "filename should end with timestamp-serve.json"
        );

        let content = std::fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(json["timestamp"], "2026-06-21T10-30-00");
        assert_eq!(json["source"], "file:///tmp/b.toml");

        let res = &json["summary"]["resources"][0];
        assert!(
            res["from"].is_null() || res.get("from").is_none(),
            "from should be redacted"
        );
        assert!(
            res["to"].is_null() || res.get("to").is_none(),
            "to should be redacted"
        );
        assert!(
            res["output"].is_null() || res.get("output").is_none(),
            "output should be redacted"
        );
    }

    #[test]
    fn write_and_read_log() {
        let dir = TempDir::new().unwrap();
        let summaries = vec![RunSummary::from_results(
            "web1",
            vec![ResourceResult {
                resource_type: "pkg".into(),
                name: "nginx".into(),
                status: ResourceStatus::Changed,
                diff: Some("installed".into()),
                from: None,
                to: None,
                error: None,
                output: None,
                changes: Vec::new(),
            }],
        )];

        write_log(dir.path(), &summaries).unwrap();

        let log_dir = dir.path().join(".verg").join("logs");
        let entries: Vec<_> = std::fs::read_dir(log_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert_eq!(entries.len(), 1);
        assert!(
            entries[0]
                .file_name()
                .to_string_lossy()
                .ends_with("-apply.json")
        );
    }
}
