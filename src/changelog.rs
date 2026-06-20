use std::path::Path;

use chrono::Utc;

use crate::error::Error;
use crate::resources::RunSummary;

/// Produce a changelog-safe copy: drop from/to/output bodies and truncate diff,
/// so the log records that a resource changed without persisting secret bodies.
pub fn redact_for_changelog(summaries: &[RunSummary]) -> Vec<RunSummary> {
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

pub fn write_log(base_dir: &Path, summaries: &[RunSummary]) -> Result<(), Error> {
    let log_dir = base_dir.join(".verg").join("logs");
    std::fs::create_dir_all(&log_dir)
        .map_err(|e| Error::Other(format!("failed to create log dir: {e}")))?;

    let timestamp = Utc::now().format("%Y-%m-%dT%H-%M-%S");
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
