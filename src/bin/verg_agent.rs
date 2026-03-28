use std::io::Read;

use verg::bundle::Bundle;
use verg::resources::dag;
use verg::resources::{self, ResourceStatus, RunSummary};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dry_run = args.iter().any(|a| a == "--dry-run");

    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("failed to read stdin: {e}");
        std::process::exit(7);
    }

    let bundle = match Bundle::from_toml(&input) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("failed to parse bundle: {e}");
            std::process::exit(5);
        }
    };

    let layers = match dag::resolve_order(&bundle.resources) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("dependency error: {e}");
            std::process::exit(5);
        }
    };

    let mut results = Vec::new();
    let mut failed_fqns = std::collections::HashSet::new();

    for layer in &layers {
        for resource in layer {
            let should_skip = resource.after.iter().any(|dep| failed_fqns.contains(dep));
            if should_skip {
                results.push(resources::ResourceResult {
                    resource_type: resource.resource_type.clone(),
                    name: resource.name.clone(),
                    status: ResourceStatus::Skipped,
                    diff: None,
                    from: None,
                    to: None,
                    error: Some("dependency failed".into()),
                });
                failed_fqns.insert(resource.fqn());
                continue;
            }

            let result = resources::execute_resource(resource, dry_run);
            if result.status == ResourceStatus::Failed {
                failed_fqns.insert(resource.fqn());
            }
            results.push(result);
        }
    }

    let summary = RunSummary::from_results(&bundle.host, results);

    match serde_json::to_string(&summary) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("failed to serialize results: {e}");
            std::process::exit(7);
        }
    }

    if summary.summary.failed > 0 && summary.summary.ok + summary.summary.changed == 0 {
        std::process::exit(3);
    } else if summary.summary.failed > 0 {
        std::process::exit(2);
    } else if summary.summary.changed == 0 {
        std::process::exit(1);
    }
}
