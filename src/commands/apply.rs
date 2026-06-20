use std::io::IsTerminal;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::engine::{Engine, EngineResult};
use crate::error::Error;
use crate::output::OutputConfig;

pub async fn run(
    engine: &Engine,
    base_dir: &Path,
    targets: &str,
    yes: bool,
    output: &OutputConfig,
    cancel: Arc<AtomicBool>,
) -> Result<i32, Error> {
    if !yes && !std::io::stdin().is_terminal() {
        return Err(Error::ConfirmationRequired(
            "apply modifies infrastructure; pass --yes to confirm non-interactively".into(),
        ));
    }
    let result = engine
        .run_cancellable(base_dir, targets, false, cancel)
        .await?;
    print_result(&result, output, &mut std::io::stdout());

    if let Err(e) = crate::changelog::write_log(base_dir, &result.summaries) {
        eprintln!("Warning: failed to write change log: {e}");
    }

    Ok(result.exit_code())
}

pub fn print_result(result: &EngineResult, output: &OutputConfig, out: &mut impl std::io::Write) {
    if output.json {
        let envelope =
            serde_json::json!({ "items": &result.summaries, "total": result.summaries.len() });
        let json = serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| "{}".to_string());
        let _ = writeln!(out, "{json}");
    } else {
        for summary in &result.summaries {
            if !output.quiet {
                for r in &summary.resources {
                    let _ = writeln!(
                        out,
                        "{}",
                        format_resource_line(&summary.host, r, output.color)
                    );
                }
            }
            let _ = writeln!(
                out,
                "{}: {} changed, {} ok, {} failed, {} skipped\n",
                summary.host,
                summary.summary.changed,
                summary.summary.ok,
                summary.summary.failed,
                summary.summary.skipped
            );
        }
    }
}

/// One human-readable apply line for a resource.
fn format_resource_line(host: &str, r: &crate::resources::ResourceResult, color: bool) -> String {
    use crate::resources::ResourceStatus;
    let (symbol, status_text) = match r.status {
        ResourceStatus::Ok => ("\u{2713}", "already ok"),
        ResourceStatus::Changed => ("\u{2717}", "changed"),
        ResourceStatus::Failed => ("\u{2717}", "FAILED"),
        ResourceStatus::Skipped => ("-", "skipped"),
    };
    let detail = match &r.diff {
        Some(d) => format!(" -> {d}"),
        None => match &r.error {
            Some(e) => format!(" ({e})"),
            None => String::new(),
        },
    };
    let symbol = if color {
        use owo_colors::OwoColorize;
        match r.status {
            ResourceStatus::Ok => symbol.green().to_string(),
            ResourceStatus::Changed => symbol.yellow().to_string(),
            ResourceStatus::Failed => symbol.red().to_string(),
            ResourceStatus::Skipped => symbol.dimmed().to_string(),
        }
    } else {
        symbol.to_string()
    };
    format!(
        "{host}: {}.{} {symbol} {status_text}{detail}",
        r.resource_type, r.name
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::{ResourceResult, ResourceStatus, RunSummary};

    fn engine_result() -> EngineResult {
        EngineResult {
            summaries: vec![RunSummary::from_results(
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
            )],
        }
    }

    #[test]
    fn resource_line_includes_host_and_status() {
        let r = ResourceResult {
            resource_type: "pkg".into(),
            name: "nginx".into(),
            status: ResourceStatus::Changed,
            diff: Some("installed".into()),
            from: None,
            to: None,
            error: None,
            output: None,
        };
        let line = format_resource_line("web1", &r, false);
        assert!(
            line.contains("web1")
                && line.contains("pkg.nginx")
                && line.contains("changed")
                && line.contains("installed"),
            "got: {line}"
        );
    }

    #[test]
    fn print_result_writes_to_the_given_writer() {
        // Text mode (not a TTY in tests), no color, quiet=false shows per-resource lines.
        let output = OutputConfig::new(crate::output::OutputFormat::Text, false, false);
        let mut buf: Vec<u8> = Vec::new();
        print_result(&engine_result(), &output, &mut buf);
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("web1"), "host missing from output: {s}");
        assert!(
            s.contains("pkg.nginx"),
            "resource line missing when quiet=false: {s}"
        );
        assert!(s.contains("changed"), "status missing: {s}");
    }

    #[test]
    fn quiet_suppresses_resource_lines_but_keeps_summary() {
        let output = OutputConfig::new(crate::output::OutputFormat::Text, false, true);
        let mut buf: Vec<u8> = Vec::new();
        print_result(&engine_result(), &output, &mut buf);
        let s = String::from_utf8(buf).unwrap();
        assert!(
            !s.contains("pkg.nginx"),
            "per-resource line must be absent when quiet=true: {s}"
        );
        assert!(
            !s.contains("installed"),
            "per-resource detail must be absent when quiet=true: {s}"
        );
        assert!(
            s.contains("1 changed"),
            "summary must still appear when quiet=true: {s}"
        );
        assert!(s.contains("web1"), "host must still appear in summary: {s}");
    }
}
