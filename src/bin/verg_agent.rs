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
    let mut services_to_restart = std::collections::HashSet::new();

    for layer in &layers {
        for resource in layer {
            // Evaluate `when` condition
            if let Some(when_expr) = &resource.when
                && !resources::when::evaluate(when_expr, &bundle.facts)
            {
                results.push(resources::ResourceResult {
                    resource_type: resource.resource_type.clone(),
                    name: resource.name.clone(),
                    status: ResourceStatus::Skipped,
                    diff: None,
                    from: None,
                    to: None,
                    output: None,
                    error: Some(format!("when: {when_expr}")),
                });
                continue;
            }

            let should_skip = resource.after.iter().any(|dep| failed_fqns.contains(dep));
            if should_skip {
                results.push(resources::ResourceResult {
                    resource_type: resource.resource_type.clone(),
                    name: resource.name.clone(),
                    status: ResourceStatus::Skipped,
                    diff: None,
                    from: None,
                    to: None,
                    output: None,
                    error: Some("dependency failed".into()),
                });
                failed_fqns.insert(resource.fqn());
                continue;
            }

            let result = resources::execute_resource(resource, dry_run);
            if result.status == ResourceStatus::Changed {
                for svc in &resource.notify {
                    services_to_restart.insert(svc.clone());
                }
            }
            if result.status == ResourceStatus::Failed {
                failed_fqns.insert(resource.fqn());
            }
            results.push(result);
        }
    }

    // Restart notified services
    for svc in &services_to_restart {
        let (restart_type, restart_cmd) = if let Some(project_dir) = svc.strip_prefix("docker:") {
            let compose_file = format!("{project_dir}/docker-compose.yml");
            (
                "docker_compose",
                format!("docker compose -f {compose_file} restart"),
            )
        } else {
            ("service", format!("systemctl restart {svc}"))
        };

        if dry_run {
            results.push(resources::ResourceResult {
                resource_type: restart_type.into(),
                name: format!("{svc} (restart)"),
                status: ResourceStatus::Changed,
                diff: Some(format!("would run: {restart_cmd}")),
                from: None,
                to: None,
                output: None,
                error: None,
            });
        } else {
            let output = std::process::Command::new("sh")
                .args(["-c", &restart_cmd])
                .output();
            match output {
                Ok(o) if o.status.success() => {
                    results.push(resources::ResourceResult {
                        resource_type: "service".into(),
                        name: format!("{svc} (restart)"),
                        status: ResourceStatus::Changed,
                        diff: Some(format!("restarted {svc}")),
                        from: None,
                        to: None,
                        output: None,
                        error: None,
                    });
                }
                Ok(o) => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    results.push(resources::ResourceResult {
                        resource_type: "service".into(),
                        name: format!("{svc} (restart)"),
                        status: ResourceStatus::Failed,
                        diff: None,
                        from: None,
                        to: None,
                        output: None,
                        error: Some(format!("restart failed: {stderr}")),
                    });
                }
                Err(e) => {
                    results.push(resources::ResourceResult {
                        resource_type: "service".into(),
                        name: format!("{svc} (restart)"),
                        status: ResourceStatus::Failed,
                        diff: None,
                        from: None,
                        to: None,
                        output: None,
                        error: Some(format!("restart failed: {e}")),
                    });
                }
            }
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
