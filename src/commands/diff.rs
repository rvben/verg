use std::path::Path;

use crate::engine::{Engine, EngineResult};
use crate::error::Error;
use crate::output::OutputConfig;
use crate::resources::ResourceStatus;

pub async fn run(
    engine: &Engine,
    base_dir: &Path,
    targets: &str,
    limit: usize,
    offset: usize,
    fields: Option<String>,
    output: &OutputConfig,
) -> Result<i32, Error> {
    let result = engine.run(base_dir, targets, true).await?;
    print_diff(&result, limit, offset, fields, output);
    // diff succeeds with exit 0 when no changes (output with changes is still success),
    // and non-zero only on actual failures (connection errors etc.)
    if result.has_failures() {
        Ok(crate::error::exit_codes::PARTIAL_FAILURE)
    } else {
        Ok(crate::error::exit_codes::SUCCESS)
    }
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
        let json = serde_json::to_string_pretty(&envelope).unwrap();
        println!("{json}");
    } else {
        if result.summaries.is_empty() {
            println!("no hosts matched");
        }
        for summary in &result.summaries {
            for r in &summary.resources {
                if r.status == ResourceStatus::Ok {
                    continue;
                }
                let detail = r.diff.as_deref().unwrap_or("");
                println!(
                    "{}: {}.{} would change: {detail}",
                    summary.host, r.resource_type, r.name
                );
            }
            if summary.summary.changed > 0 {
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
