use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::engine::{Engine, EngineResult};
use crate::error::Error;
use crate::output::OutputConfig;

pub struct DiffOptions {
    pub limit: usize,
    pub offset: usize,
    pub fields: Option<String>,
}

pub async fn run(
    engine: &Engine,
    base_dir: &Path,
    targets: &str,
    opts: DiffOptions,
    output: &OutputConfig,
    cancel: Arc<AtomicBool>,
) -> Result<i32, Error> {
    let result = engine
        .run_cancellable(base_dir, targets, true, cancel)
        .await?;
    print_diff(&result, opts.limit, opts.offset, opts.fields, output);
    // diff succeeds with exit 0 when no changes (output with changes is still success),
    // and non-zero only on actual failures (connection errors etc.)
    if result.has_failures() {
        if result.is_connection_only_failure() {
            Ok(crate::error::exit_codes::CONNECTION_ERROR)
        } else {
            Ok(crate::error::exit_codes::PARTIAL_FAILURE)
        }
    } else {
        Ok(crate::error::exit_codes::SUCCESS)
    }
}

/// One human-readable diff line for a resource.
fn diff_line(host: &str, r: &crate::resources::ResourceResult) -> String {
    use crate::resources::ResourceStatus;
    let fqn = format!("{}.{}", r.resource_type, r.name);
    match r.status {
        ResourceStatus::Failed => {
            let err = r.error.as_deref().unwrap_or("failed");
            format!("{host}: {fqn} FAILED: {err}")
        }
        _ => match r.diff.as_deref() {
            Some(d) if !d.is_empty() => format!("{host}: {fqn} would change: {d}"),
            _ => format!("{host}: {fqn} would change"),
        },
    }
}

/// Whether a resource represents a pending change to show in diff (Ok/Skipped do not).
fn is_pending_change(status: crate::resources::ResourceStatus) -> bool {
    use crate::resources::ResourceStatus;
    matches!(status, ResourceStatus::Changed | ResourceStatus::Failed)
}

fn print_diff(
    result: &EngineResult,
    limit: usize,
    offset: usize,
    fields: Option<String>,
    output: &OutputConfig,
) {
    if output.json {
        let total = result.summaries.len();
        let mut envelope = serde_json::json!({
            "items": &result.summaries,
            "total": total,
            "limit": limit,
            "offset": offset
        });
        if let Some(f) = &fields {
            envelope["fields"] = serde_json::Value::String(f.clone());
        }
        let json = serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| "{}".to_string());
        println!("{json}");
    } else {
        if result.summaries.is_empty() {
            println!("no hosts matched");
        }
        for summary in &result.summaries {
            for r in &summary.resources {
                if !is_pending_change(r.status.clone()) {
                    continue;
                }
                println!("{}", diff_line(&summary.host, r));
            }
            if summary.summary.failed > 0 {
                println!(
                    "{}: {} would change, {} FAILED",
                    summary.host, summary.summary.changed, summary.summary.failed
                );
            } else if summary.summary.changed > 0 {
                println!(
                    "{}: {} resource(s) would change",
                    summary.host, summary.summary.changed
                );
            } else {
                println!("{}: no changes needed", summary.host);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{ResourceResult, ResourceStatus};

    fn res(status: ResourceStatus, diff: Option<&str>, error: Option<&str>) -> ResourceResult {
        ResourceResult {
            resource_type: "file".into(),
            name: "x".into(),
            status,
            diff: diff.map(String::from),
            from: None,
            to: None,
            error: error.map(String::from),
            output: None,
        }
    }

    #[test]
    fn failed_resource_shows_failed_not_would_change() {
        let line = diff_line(
            "h",
            &res(ResourceStatus::Failed, None, Some("connection refused")),
        );
        assert!(line.contains("FAILED"), "got: {line}");
        assert!(line.contains("connection refused"), "got: {line}");
        assert!(!line.contains("would change"), "got: {line}");
    }

    #[test]
    fn changed_resource_shows_would_change() {
        let line = diff_line("h", &res(ResourceStatus::Changed, Some("mode 0644"), None));
        assert!(line.contains("would change"), "got: {line}");
        assert!(line.contains("mode 0644"), "got: {line}");
    }

    #[test]
    fn changed_without_detail_has_no_trailing_colon() {
        let line = diff_line("h", &res(ResourceStatus::Changed, None, None));
        assert!(!line.ends_with(": "), "trailing colon: {line:?}");
        assert!(!line.contains("would change: "), "empty detail: {line:?}");
    }

    #[test]
    fn ok_and_skipped_are_not_printed() {
        // The loop skips Ok and Skipped (neither is a pending change).
        assert!(!is_pending_change(ResourceStatus::Ok));
        assert!(!is_pending_change(ResourceStatus::Skipped));
        assert!(is_pending_change(ResourceStatus::Changed));
        assert!(is_pending_change(ResourceStatus::Failed));
    }
}
