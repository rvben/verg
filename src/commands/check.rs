use std::path::Path;

use crate::engine::Engine;
use crate::error::{self, Error};
use crate::output::OutputConfig;

pub async fn run(
    engine: &Engine,
    base_dir: &Path,
    targets: &str,
    output: &OutputConfig,
) -> Result<i32, Error> {
    let result = engine.run(base_dir, targets, true).await?;
    if output.json {
        let json = serde_json::to_string_pretty(&result.summaries).unwrap();
        println!("{json}");
    }
    Ok(if result.has_changes() {
        error::exit_codes::NOTHING_CHANGED
    } else {
        error::exit_codes::SUCCESS
    })
}
