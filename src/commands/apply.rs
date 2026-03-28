use std::path::Path;

use crate::engine::{Engine, EngineResult};
use crate::error::{self, Error};
use crate::output::OutputConfig;
use crate::resources::ResourceStatus;

pub async fn run(
    engine: &Engine,
    base_dir: &Path,
    targets: &str,
    output: &OutputConfig,
) -> Result<i32, Error> {
    let result = engine.run(base_dir, targets, false).await?;
    print_result(&result, output);

    if let Err(e) = crate::changelog::write_log(base_dir, &result.summaries) {
        eprintln!("Warning: failed to write change log: {e}");
    }

    Ok(exit_code(&result))
}

pub fn print_result(result: &EngineResult, output: &OutputConfig) {
    if output.json {
        let json = serde_json::to_string_pretty(&result.summaries).unwrap();
        println!("{json}");
    } else {
        for summary in &result.summaries {
            for r in &summary.resources {
                let symbol = match r.status {
                    ResourceStatus::Ok => "\x1b[32m✓\x1b[0m",
                    ResourceStatus::Changed => "\x1b[33m✗\x1b[0m",
                    ResourceStatus::Failed => "\x1b[31m✗\x1b[0m",
                    ResourceStatus::Skipped => "\x1b[90m-\x1b[0m",
                };
                let detail = match &r.diff {
                    Some(d) => format!(" → {d}"),
                    None => match &r.error {
                        Some(e) => format!(" ({e})"),
                        None => String::new(),
                    },
                };
                let status_text = match r.status {
                    ResourceStatus::Ok => "already ok",
                    ResourceStatus::Changed => "changed",
                    ResourceStatus::Failed => "FAILED",
                    ResourceStatus::Skipped => "skipped",
                };
                eprintln!(
                    "{}: {}.{} {symbol} {status_text}{detail}",
                    summary.host, r.resource_type, r.name
                );
            }
            eprintln!(
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

fn exit_code(result: &EngineResult) -> i32 {
    if result.has_failures() {
        if result.has_changes() || result.summaries.iter().any(|s| s.summary.ok > 0) {
            error::exit_codes::PARTIAL_FAILURE
        } else {
            error::exit_codes::TOTAL_FAILURE
        }
    } else if result.has_changes() {
        error::exit_codes::SUCCESS
    } else {
        error::exit_codes::NOTHING_CHANGED
    }
}
