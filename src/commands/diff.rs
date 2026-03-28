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
    let result = engine.run(base_dir, targets, true).await?;
    print_diff(&result, output);
    Ok(if result.has_changes() {
        error::exit_codes::SUCCESS
    } else {
        error::exit_codes::NOTHING_CHANGED
    })
}

fn print_diff(result: &EngineResult, output: &OutputConfig) {
    if output.json {
        let json = serde_json::to_string_pretty(&result.summaries).unwrap();
        println!("{json}");
    } else {
        for summary in &result.summaries {
            for r in &summary.resources {
                if r.status == ResourceStatus::Ok {
                    continue;
                }
                let detail = r.diff.as_deref().unwrap_or("");
                eprintln!(
                    "{}: {}.{} would change: {detail}",
                    summary.host, r.resource_type, r.name
                );
            }
            if summary.summary.changed > 0 {
                eprintln!(
                    "{}: {} resource(s) would change\n",
                    summary.host, summary.summary.changed
                );
            } else {
                eprintln!("{}: no changes needed\n", summary.host);
            }
        }
    }
}
