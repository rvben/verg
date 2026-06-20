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

/// Page a slice by offset/limit, clamping to bounds.
fn paginate<T>(items: &[T], limit: usize, offset: usize) -> &[T] {
    let start = offset.min(items.len());
    let end = start.saturating_add(limit).min(items.len());
    &items[start..end]
}

/// Project a resource result's JSON to `type`/`name` plus the requested fields.
fn project_resource(
    r: &crate::resources::ResourceResult,
    keep: &std::collections::HashSet<&str>,
) -> serde_json::Value {
    match serde_json::to_value(r).unwrap_or(serde_json::Value::Null) {
        serde_json::Value::Object(map) => serde_json::Value::Object(
            map.into_iter()
                .filter(|(k, _)| k == "type" || k == "name" || keep.contains(k.as_str()))
                .collect(),
        ),
        other => other,
    }
}

fn write_diff(
    result: &EngineResult,
    limit: usize,
    offset: usize,
    fields: Option<String>,
    output: &OutputConfig,
    out: &mut impl std::io::Write,
) {
    let total = result.summaries.len();
    let page = paginate(&result.summaries, limit, offset);
    if output.json {
        let items: serde_json::Value =
            if let Some(f) = &fields {
                let keep: std::collections::HashSet<&str> = f.split(',').map(str::trim).collect();
                serde_json::Value::Array(page.iter().map(|s| {
                let res: Vec<serde_json::Value> =
                    s.resources.iter().map(|r| project_resource(r, &keep)).collect();
                serde_json::json!({"host": s.host, "summary": s.summary, "resources": res})
            }).collect())
            } else {
                serde_json::to_value(page).unwrap_or(serde_json::Value::Null)
            };
        let envelope =
            serde_json::json!({"items": items, "total": total, "limit": limit, "offset": offset});
        let json = serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| "{}".to_string());
        let _ = writeln!(out, "{json}");
    } else {
        if page.is_empty() {
            let _ = writeln!(out, "no hosts matched");
        }
        for summary in page {
            if !output.quiet {
                for r in &summary.resources {
                    if !is_pending_change(r.status.clone()) {
                        continue;
                    }
                    let _ = writeln!(out, "{}", diff_line(&summary.host, r));
                }
            }
            if summary.summary.failed > 0 {
                let _ = writeln!(
                    out,
                    "{}: {} would change, {} FAILED",
                    summary.host, summary.summary.changed, summary.summary.failed
                );
            } else if summary.summary.changed > 0 {
                let _ = writeln!(
                    out,
                    "{}: {} resource(s) would change",
                    summary.host, summary.summary.changed
                );
            } else {
                let _ = writeln!(out, "{}: no changes needed", summary.host);
            }
        }
    }
}

fn print_diff(
    result: &EngineResult,
    limit: usize,
    offset: usize,
    fields: Option<String>,
    output: &OutputConfig,
) {
    write_diff(
        result,
        limit,
        offset,
        fields,
        output,
        &mut std::io::stdout(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{ResourceResult, ResourceStatus, RunSummary};

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

    fn engine_result_with_change() -> EngineResult {
        EngineResult {
            summaries: vec![RunSummary::from_results(
                "web1",
                vec![res(ResourceStatus::Changed, Some("mode 0755"), None)],
            )],
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

    #[test]
    fn paginate_slices_summaries() {
        let s: Vec<u32> = (0..10).collect();
        assert_eq!(paginate(&s, 3, 2), &s[2..5]);
        assert_eq!(paginate(&s, 100, 8), &s[8..10]); // limit past end clamps
        assert_eq!(paginate(&s, 3, 50), &[] as &[u32]); // offset past end -> empty
    }

    #[test]
    fn project_resource_keeps_type_name_and_requested_only() {
        use std::collections::HashSet;
        let r = ResourceResult {
            resource_type: "file".into(),
            name: "conf".into(),
            status: ResourceStatus::Changed,
            diff: Some("mode".into()),
            from: Some("old".into()),
            to: None,
            error: None,
            output: None,
        };
        let keep: HashSet<&str> = ["diff"].into_iter().collect();
        let v = project_resource(&r, &keep);
        let obj = v.as_object().unwrap();
        assert!(obj.contains_key("type"), "type always kept"); // serde rename of resource_type
        assert!(obj.contains_key("name"), "name always kept");
        assert!(obj.contains_key("diff"), "requested field kept");
        assert!(!obj.contains_key("status"), "unrequested field dropped");
        assert!(!obj.contains_key("from"), "unrequested field dropped");
    }

    #[test]
    fn write_diff_shows_resource_lines_when_not_quiet() {
        let output =
            crate::output::OutputConfig::new(crate::output::OutputFormat::Text, false, false);
        let mut buf: Vec<u8> = Vec::new();
        write_diff(
            &engine_result_with_change(),
            100,
            0,
            None,
            &output,
            &mut buf,
        );
        let s = String::from_utf8(buf).unwrap();
        // Assert on the per-resource line's exclusive content ("file.x" and the
        // "mode 0755" detail) - these appear only in the per-resource diff line,
        // not the summary, so the assertion fails if the quiet guard is removed.
        assert!(
            s.contains("file.x"),
            "per-resource name must appear when quiet=false: {s}"
        );
        assert!(
            s.contains("mode 0755"),
            "per-resource detail must appear when quiet=false: {s}"
        );
        assert!(
            s.contains("resource(s) would change"),
            "summary line must appear: {s}"
        );
    }

    #[test]
    fn write_diff_quiet_suppresses_resource_lines_keeps_summary() {
        let output =
            crate::output::OutputConfig::new(crate::output::OutputFormat::Text, false, true);
        let mut buf: Vec<u8> = Vec::new();
        write_diff(
            &engine_result_with_change(),
            100,
            0,
            None,
            &output,
            &mut buf,
        );
        let s = String::from_utf8(buf).unwrap();
        // The per-resource diff line contains "file.x would change: mode 0755" -
        // that detail must be absent; only the summary "resource(s) would change" remains.
        assert!(
            !s.contains("file.x"),
            "per-resource name must be absent when quiet=true: {s}"
        );
        assert!(
            !s.contains("mode 0755"),
            "per-resource detail must be absent when quiet=true: {s}"
        );
        assert!(
            s.contains("resource(s) would change"),
            "summary line must still appear when quiet=true: {s}"
        );
    }
}
