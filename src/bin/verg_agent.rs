use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::process::Command as ProcessCommand;

use verg::bundle::Bundle;
use verg::resources::dag;
use verg::resources::{
    self, REGISTER_SENTINEL, REGISTER_SENTINEL_END, ResolvedResource, ResourceResult,
    ResourceStatus, RunSummary,
};

/// Replace register sentinel tokens in resource string props with actual values.
fn interpolate_registers(
    resource: &ResolvedResource,
    registers: &HashMap<String, String>,
) -> ResolvedResource {
    let mut res = resource.clone();
    for value in res.props.values_mut() {
        if let toml::Value::String(s) = value {
            let mut result = s.clone();
            loop {
                let Some(start) = result.find(REGISTER_SENTINEL) else {
                    break;
                };
                let after_prefix = start + REGISTER_SENTINEL.len();
                let Some(end) = result[after_prefix..].find(REGISTER_SENTINEL_END) else {
                    break;
                };
                let name = &result[after_prefix..after_prefix + end];
                let sentinel = format!("{REGISTER_SENTINEL}{name}{REGISTER_SENTINEL_END}");
                if let Some(val) = registers.get(name) {
                    result = result.replacen(&sentinel, val, 1);
                } else {
                    // Leave sentinel in place if register not yet available
                    break;
                }
            }
            *s = result;
        }
    }
    res
}

/// Execute a handler resource, bypassing guards (creates/unless/onlyif).
fn execute_handler(resource: &ResolvedResource, dry_run: bool) -> ResourceResult {
    let mut res = resource.clone();
    res.props.remove("creates");
    res.props.remove("unless");
    res.props.remove("onlyif");
    if res.resource_type == "cmd" {
        res.props
            .insert("onlyif".into(), toml::Value::String("true".into()));
    }
    let mut result = resources::execute_resource(&res, dry_run);
    result.name = format!("{} (handler)", result.name);
    result
}

/// Validate that a service name contains only safe characters.
fn is_valid_service_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || c == '.' || c == '-' || c == '_' || c == '@')
}

/// Return (resource_type, description) for a shorthand notify target.
fn describe_notify(target: &str) -> (&str, String) {
    if target == "daemon-reload" {
        ("service", "systemd daemon-reload".into())
    } else if let Some(svc) = target.strip_prefix("restart:") {
        ("service", format!("{svc} (restart)"))
    } else if let Some(svc) = target.strip_prefix("reload:") {
        ("service", format!("{svc} (reload)"))
    } else if let Some(path) = target.strip_prefix("docker-restart:") {
        ("docker_compose", format!("{path} (restart)"))
    } else if let Some(path) = target.strip_prefix("docker-up:") {
        ("docker_compose", format!("{path} (up)"))
    } else {
        ("notify", format!("{target} (notify)"))
    }
}

/// Run the actual command for a shorthand notify target. Uses Command::new with args (no sh -c).
fn run_notify_command(target: &str) -> Result<std::process::Output, std::io::Error> {
    if target == "daemon-reload" {
        ProcessCommand::new("systemctl")
            .args(["daemon-reload"])
            .output()
    } else if let Some(svc) = target.strip_prefix("restart:") {
        ProcessCommand::new("systemctl")
            .args(["restart", svc])
            .output()
    } else if let Some(svc) = target.strip_prefix("reload:") {
        ProcessCommand::new("systemctl")
            .args(["reload", svc])
            .output()
    } else if let Some(path) = target.strip_prefix("docker-restart:") {
        ProcessCommand::new("docker")
            .args([
                "compose",
                "-f",
                &format!("{path}/docker-compose.yml"),
                "restart",
            ])
            .output()
    } else if let Some(path) = target.strip_prefix("docker-up:") {
        ProcessCommand::new("docker")
            .args([
                "compose",
                "-f",
                &format!("{path}/docker-compose.yml"),
                "up",
                "-d",
            ])
            .output()
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unknown notify target: {target}"),
        ))
    }
}

/// Execute a shorthand notify action (restart:X, reload:X, daemon-reload, docker-restart:X, docker-up:X).
fn execute_notify_shorthand(target: &str, dry_run: bool) -> ResourceResult {
    let (resource_type, description) = describe_notify(target);

    // Validate service names for systemctl-based actions
    if let Some(svc) = target
        .strip_prefix("restart:")
        .or_else(|| target.strip_prefix("reload:"))
        && !is_valid_service_name(svc)
    {
        return ResourceResult {
            resource_type: resource_type.into(),
            name: description,
            status: ResourceStatus::Failed,
            diff: None,
            from: None,
            to: None,
            output: None,
            error: Some(format!("invalid service name: {svc}")),
        };
    }

    if dry_run {
        return ResourceResult {
            resource_type: resource_type.into(),
            name: description,
            status: ResourceStatus::Changed,
            diff: Some(format!("would run: {target}")),
            from: None,
            to: None,
            output: None,
            error: None,
        };
    }

    match run_notify_command(target) {
        Ok(o) if o.status.success() => ResourceResult {
            resource_type: resource_type.into(),
            name: description,
            status: ResourceStatus::Changed,
            diff: Some(format!("executed: {target}")),
            from: None,
            to: None,
            output: None,
            error: None,
        },
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            ResourceResult {
                resource_type: resource_type.into(),
                name: description,
                status: ResourceStatus::Failed,
                diff: None,
                from: None,
                to: None,
                output: None,
                error: Some(format!("notify failed: {stderr}")),
            }
        }
        Err(e) => ResourceResult {
            resource_type: resource_type.into(),
            name: description,
            status: ResourceStatus::Failed,
            diff: None,
            from: None,
            to: None,
            output: None,
            error: Some(format!("notify failed: {e}")),
        },
    }
}

/// Classify a notify target: if it matches a handler FQN (type.name), it's a handler reference;
/// otherwise it's a shorthand action.
fn is_handler_fqn(target: &str, handler_fqns: &HashSet<String>) -> bool {
    handler_fqns.contains(target)
}

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

    // Partition resources into normal vs handler
    let (normal_resources, handler_resources): (Vec<ResolvedResource>, Vec<ResolvedResource>) =
        bundle.resources.into_iter().partition(|r| !r.handler);

    // Build set of handler FQNs for notify classification
    let handler_fqns: HashSet<String> = handler_resources.iter().map(|r| r.fqn()).collect();

    // Resolve execution order for normal resources only
    let layers = match dag::resolve_order(&normal_resources) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("dependency error: {e}");
            std::process::exit(5);
        }
    };

    let mut results = Vec::new();
    let mut failed_fqns = HashSet::new();
    let mut registers: HashMap<String, String> = HashMap::new();
    let mut notified_handlers: HashSet<String> = HashSet::new();
    let mut notified_shorthands: Vec<String> = Vec::new();
    let mut shorthand_seen: HashSet<String> = HashSet::new();

    // Execute normal resources in DAG order
    for layer in &layers {
        for resource in layer {
            // Evaluate `when` condition
            if let Some(when_expr) = &resource.when
                && !resources::when::evaluate(when_expr, &bundle.facts)
            {
                results.push(ResourceResult {
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

            // Skip if any dependency failed
            let should_skip = resource.after.iter().any(|dep| failed_fqns.contains(dep));
            if should_skip {
                results.push(ResourceResult {
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

            // Interpolate register sentinel tokens
            let interpolated = interpolate_registers(resource, &registers);
            let result = resources::execute_resource(&interpolated, dry_run);

            // Capture register output
            if let Some(ref reg_name) = resource.register
                && let Some(ref output) = result.output
            {
                registers.insert(reg_name.clone(), output.clone());
            }

            // Collect notify targets on change
            if result.status == ResourceStatus::Changed {
                for target in &resource.notify {
                    if is_handler_fqn(target, &handler_fqns) {
                        notified_handlers.insert(target.clone());
                    } else if shorthand_seen.insert(target.clone()) {
                        notified_shorthands.push(target.clone());
                    }
                }
            }

            if result.status == ResourceStatus::Failed {
                failed_fqns.insert(resource.fqn());
            }

            results.push(result);
        }
    }

    // Execute notified handlers (with guard bypass)
    if !notified_handlers.is_empty() {
        let triggered_handlers: Vec<ResolvedResource> = handler_resources
            .into_iter()
            .filter(|r| notified_handlers.contains(&r.fqn()))
            .collect();

        match dag::resolve_order(&triggered_handlers) {
            Ok(handler_layers) => {
                for layer in &handler_layers {
                    for resource in layer {
                        let interpolated = interpolate_registers(resource, &registers);
                        let result = execute_handler(&interpolated, dry_run);
                        results.push(result);
                    }
                }
            }
            Err(e) => {
                eprintln!("handler dependency error: {e}");
                results.push(ResourceResult {
                    resource_type: "handler".into(),
                    name: "dependency resolution".into(),
                    status: ResourceStatus::Failed,
                    diff: None,
                    from: None,
                    to: None,
                    output: None,
                    error: Some(format!("handler dependency error: {e}")),
                });
            }
        }
    }

    // Execute shorthand notify actions
    for target in &notified_shorthands {
        results.push(execute_notify_shorthand(target, dry_run));
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
