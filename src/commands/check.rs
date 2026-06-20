use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::engine::Engine;
use crate::error::Error;
use crate::output::OutputConfig;

pub async fn run(
    engine: &Engine,
    base_dir: &Path,
    targets: &str,
    output: &OutputConfig,
    cancel: Arc<AtomicBool>,
) -> Result<i32, Error> {
    let result = engine
        .run_cancellable(base_dir, targets, true, cancel)
        .await?;
    if output.json {
        let envelope = serde_json::json!({
            "items": &result.summaries,
            "total": result.summaries.len()
        });
        let json = serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| "{}".to_string());
        println!("{json}");
    } else {
        for s in &result.summaries {
            println!("{}", check_line(s));
        }
    }
    Ok(result.exit_code())
}

fn check_line(s: &crate::resources::RunSummary) -> String {
    use crate::resources::ResourceStatus;
    if s.summary.failed > 0 {
        // Surface the first failure's error for actionability.
        let err = s
            .resources
            .iter()
            .find(|r| r.status == ResourceStatus::Failed)
            .and_then(|r| r.error.as_deref())
            .unwrap_or("failed");
        format!("{}: FAILED - {err}", s.host)
    } else if s.summary.changed > 0 {
        format!("{}: {} drift", s.host, s.summary.changed)
    } else {
        format!("{}: ok", s.host)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{ResourceResult, ResourceStatus, RunSummary};

    fn summary(host: &str, statuses: &[ResourceStatus]) -> RunSummary {
        let resources = statuses
            .iter()
            .map(|s| ResourceResult {
                resource_type: "pkg".into(),
                name: "x".into(),
                status: s.clone(),
                diff: None,
                from: None,
                to: None,
                error: Some("e".into()),
                output: None,
            })
            .collect();
        RunSummary::from_results(host, resources)
    }

    #[test]
    fn check_line_reports_status() {
        assert!(check_line(&summary("h", &[ResourceStatus::Ok])).contains("ok"));
        assert!(check_line(&summary("h", &[ResourceStatus::Changed])).contains("drift"));
        let failed = check_line(&summary("h", &[ResourceStatus::Failed]));
        assert!(failed.contains("FAILED"), "got: {failed}");
        assert!(
            failed.contains("e"),
            "should include the error text: {failed}"
        );
    }
}
