use std::path::Path;

use chrono::Utc;

use crate::error::Error;
use crate::resources::RunSummary;

pub fn write_log(base_dir: &Path, summaries: &[RunSummary]) -> Result<(), Error> {
    let log_dir = base_dir.join(".verg").join("logs");
    std::fs::create_dir_all(&log_dir)
        .map_err(|e| Error::Other(format!("failed to create log dir: {e}")))?;

    let timestamp = Utc::now().format("%Y-%m-%dT%H-%M-%S");
    let filename = format!("{timestamp}-apply.json");
    let path = log_dir.join(filename);

    let json = serde_json::to_string_pretty(summaries)
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
