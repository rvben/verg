use std::collections::{HashMap, HashSet};
use std::process::Command as ProcessCommand;

use crate::bundle::Bundle;
use crate::error::Error;
use crate::resources::{self, ResolvedResource, ResourceResult, ResourceStatus, RunSummary, dag};

pub use notify::{
    NotifyTarget, describe_notify, has_unresolved_registers, interpolate_registers,
    is_valid_service_name, parse_notify_target, validate_docker_path,
};

mod notify {
    use std::collections::HashMap;

    use crate::resources::{REGISTER_SENTINEL, REGISTER_SENTINEL_END, ResolvedResource};

    /// Replace register sentinel tokens in resource string props with actual values.
    pub fn interpolate_registers(
        resource: &ResolvedResource,
        registers: &HashMap<String, String>,
    ) -> ResolvedResource {
        let mut res = resource.clone();
        for value in res.props.values_mut() {
            if let toml::Value::String(s) = value {
                let mut result = s.clone();
                while let Some(start) = result.find(REGISTER_SENTINEL) {
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

    /// Check whether any string props still contain unresolved register sentinels.
    pub fn has_unresolved_registers(resource: &ResolvedResource) -> bool {
        resource.props.values().any(|v| {
            if let toml::Value::String(s) = v {
                s.contains(REGISTER_SENTINEL)
            } else {
                false
            }
        })
    }

    /// Validate that a service name contains only safe characters.
    pub fn is_valid_service_name(name: &str) -> bool {
        !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_alphanumeric() || c == '.' || c == '-' || c == '_' || c == '@')
    }

    /// Parsed notify target types.
    #[derive(Debug, PartialEq)]
    pub enum NotifyTarget<'a> {
        DaemonReload,
        Restart(&'a str),
        Reload(&'a str),
        DockerRestart(&'a str),
        DockerUp(&'a str),
        Unknown(&'a str),
    }

    /// Parse a notify target string into a structured enum.
    ///
    /// Recognized prefixes: `restart:`, `reload:`, `daemon-reload`, `docker-restart:`,
    /// `docker-up:`, `docker:` (alias for `docker-restart:`).
    /// Bare names without a prefix are treated as `Restart` (systemctl restart).
    pub fn parse_notify_target(target: &str) -> NotifyTarget<'_> {
        if target == "daemon-reload" {
            NotifyTarget::DaemonReload
        } else if let Some(svc) = target.strip_prefix("restart:") {
            NotifyTarget::Restart(svc)
        } else if let Some(svc) = target.strip_prefix("reload:") {
            NotifyTarget::Reload(svc)
        } else if let Some(path) = target.strip_prefix("docker-restart:") {
            NotifyTarget::DockerRestart(path)
        } else if let Some(path) = target.strip_prefix("docker:") {
            // Legacy alias for docker-restart:
            NotifyTarget::DockerRestart(path)
        } else if let Some(path) = target.strip_prefix("docker-up:") {
            NotifyTarget::DockerUp(path)
        } else if is_valid_service_name(target) {
            // Bare service name without prefix -> systemctl restart
            NotifyTarget::Restart(target)
        } else {
            NotifyTarget::Unknown(target)
        }
    }

    /// Return (resource_type, description) for a shorthand notify target.
    pub fn describe_notify(target: &str) -> (&str, String) {
        match parse_notify_target(target) {
            NotifyTarget::DaemonReload => ("service", "systemd daemon-reload".into()),
            NotifyTarget::Restart(svc) => ("service", format!("{svc} (restart)")),
            NotifyTarget::Reload(svc) => ("service", format!("{svc} (reload)")),
            NotifyTarget::DockerRestart(path) => ("docker_compose", format!("{path} (restart)")),
            NotifyTarget::DockerUp(path) => ("docker_compose", format!("{path} (up)")),
            NotifyTarget::Unknown(t) => ("notify", format!("{t} (notify)")),
        }
    }

    /// Validate a docker path (must be absolute).
    pub fn validate_docker_path(path: &str) -> Result<(), String> {
        if !path.starts_with('/') {
            return Err(format!(
                "docker path must be absolute (start with /): {path}"
            ));
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn make_resource(props: &[(&str, &str)]) -> ResolvedResource {
            let mut p = HashMap::new();
            for (k, v) in props {
                p.insert(k.to_string(), toml::Value::String(v.to_string()));
            }
            ResolvedResource {
                resource_type: "cmd".into(),
                name: "test".into(),
                props: p,
                after: vec![],
                notify: vec![],
                when: None,
                handler: false,
                register: None,
                sensitive: false,
            }
        }

        // --- interpolate_registers ---

        #[test]
        fn interpolate_registers_replaces_matching_sentinel() {
            let resource = make_resource(&[("content", "__VERG_REG_ip__VERG_END__")]);
            let mut registers = HashMap::new();
            registers.insert("ip".into(), "192.0.2.1".into());

            let result = interpolate_registers(&resource, &registers);
            assert_eq!(result.props["content"].as_str().unwrap(), "192.0.2.1");
        }

        #[test]
        fn interpolate_registers_leaves_missing_register() {
            let resource = make_resource(&[("content", "__VERG_REG_missing__VERG_END__")]);
            let registers = HashMap::new();

            let result = interpolate_registers(&resource, &registers);
            assert_eq!(
                result.props["content"].as_str().unwrap(),
                "__VERG_REG_missing__VERG_END__"
            );
        }

        #[test]
        fn interpolate_registers_replaces_multiple_sentinels() {
            let resource = make_resource(&[(
                "content",
                "__VERG_REG_a__VERG_END__:__VERG_REG_b__VERG_END__",
            )]);
            let mut registers = HashMap::new();
            registers.insert("a".into(), "X".into());
            registers.insert("b".into(), "Y".into());

            let result = interpolate_registers(&resource, &registers);
            assert_eq!(result.props["content"].as_str().unwrap(), "X:Y");
        }

        #[test]
        fn interpolate_registers_no_sentinels_unchanged() {
            let resource = make_resource(&[("content", "plain text")]);
            let registers = HashMap::new();

            let result = interpolate_registers(&resource, &registers);
            assert_eq!(result.props["content"].as_str().unwrap(), "plain text");
        }

        // --- has_unresolved_registers ---

        #[test]
        fn has_unresolved_registers_detects_sentinel() {
            let resource = make_resource(&[("content", "__VERG_REG_ip__VERG_END__")]);
            assert!(has_unresolved_registers(&resource));
        }

        #[test]
        fn has_unresolved_registers_false_for_clean() {
            let resource = make_resource(&[("content", "192.0.2.1")]);
            assert!(!has_unresolved_registers(&resource));
        }

        // --- is_valid_service_name ---

        #[test]
        fn valid_service_names() {
            assert!(is_valid_service_name("nginx"));
            assert!(is_valid_service_name("nginx.service"));
            assert!(is_valid_service_name("my-service"));
            assert!(is_valid_service_name("my_service"));
            assert!(is_valid_service_name("user@1000"));
        }

        #[test]
        fn invalid_service_names() {
            assert!(!is_valid_service_name(""));
            assert!(!is_valid_service_name("svc;rm -rf /"));
            assert!(!is_valid_service_name("/etc/passwd"));
            assert!(!is_valid_service_name("svc name"));
            assert!(!is_valid_service_name("svc&bg"));
        }

        // --- describe_notify ---

        #[test]
        fn describe_notify_daemon_reload() {
            let (rt, desc) = describe_notify("daemon-reload");
            assert_eq!(rt, "service");
            assert_eq!(desc, "systemd daemon-reload");
        }

        #[test]
        fn describe_notify_restart() {
            let (rt, desc) = describe_notify("restart:nginx");
            assert_eq!(rt, "service");
            assert_eq!(desc, "nginx (restart)");
        }

        #[test]
        fn describe_notify_reload() {
            let (rt, desc) = describe_notify("reload:nginx");
            assert_eq!(rt, "service");
            assert_eq!(desc, "nginx (reload)");
        }

        #[test]
        fn describe_notify_docker_restart() {
            let (rt, desc) = describe_notify("docker-restart:/opt/app");
            assert_eq!(rt, "docker_compose");
            assert_eq!(desc, "/opt/app (restart)");
        }

        #[test]
        fn describe_notify_docker_up() {
            let (rt, desc) = describe_notify("docker-up:/opt/app");
            assert_eq!(rt, "docker_compose");
            assert_eq!(desc, "/opt/app (up)");
        }

        #[test]
        fn describe_notify_bare_service_name() {
            let (rt, desc) = describe_notify("nginx");
            assert_eq!(rt, "service");
            assert_eq!(desc, "nginx (restart)");
        }

        #[test]
        fn describe_notify_docker_legacy_prefix() {
            let (rt, desc) = describe_notify("docker:/opt/ntfy");
            assert_eq!(rt, "docker_compose");
            assert_eq!(desc, "/opt/ntfy (restart)");
        }

        // --- parse_notify_target ---

        #[test]
        fn parse_notify_target_variants() {
            assert_eq!(
                parse_notify_target("daemon-reload"),
                NotifyTarget::DaemonReload
            );
            assert_eq!(
                parse_notify_target("restart:nginx"),
                NotifyTarget::Restart("nginx")
            );
            assert_eq!(
                parse_notify_target("reload:sshd"),
                NotifyTarget::Reload("sshd")
            );
            assert_eq!(
                parse_notify_target("docker-restart:/opt/app"),
                NotifyTarget::DockerRestart("/opt/app")
            );
            assert_eq!(
                parse_notify_target("docker-up:/srv/web"),
                NotifyTarget::DockerUp("/srv/web")
            );
            // Bare service names -> Restart
            assert_eq!(parse_notify_target("nginx"), NotifyTarget::Restart("nginx"));
            assert_eq!(
                parse_notify_target("node_exporter"),
                NotifyTarget::Restart("node_exporter")
            );
            // Legacy docker: prefix -> DockerRestart
            assert_eq!(
                parse_notify_target("docker:/opt/app"),
                NotifyTarget::DockerRestart("/opt/app")
            );
            // Invalid service name -> Unknown
            assert_eq!(
                parse_notify_target("svc;rm -rf /"),
                NotifyTarget::Unknown("svc;rm -rf /")
            );
        }

        // --- validate_docker_path ---

        #[test]
        fn validate_docker_path_absolute() {
            assert!(validate_docker_path("/opt/app").is_ok());
            assert!(validate_docker_path("/").is_ok());
        }

        #[test]
        fn validate_docker_path_relative_rejected() {
            assert!(validate_docker_path("opt/app").is_err());
            assert!(validate_docker_path("./app").is_err());
            assert!(validate_docker_path("").is_err());
        }
    }
}

/// Execute a handler resource, bypassing guard requirements.
fn execute_handler(resource: &ResolvedResource, dry_run: bool) -> ResourceResult {
    let mut result = resources::execute_resource(resource, dry_run, true);
    result.name = format!("{} (handler)", result.name);
    result
}

/// Run the actual command for a shorthand notify target. Uses Command::new with args (no sh -c).
fn run_notify_command(target: &str) -> Result<std::process::Output, std::io::Error> {
    match parse_notify_target(target) {
        NotifyTarget::DaemonReload => ProcessCommand::new("systemctl")
            .args(["daemon-reload"])
            .env("PATH", crate::resources::SECURE_PATH)
            .output(),
        NotifyTarget::Restart(svc) => ProcessCommand::new("systemctl")
            .args(["restart", svc])
            .env("PATH", crate::resources::SECURE_PATH)
            .output(),
        NotifyTarget::Reload(svc) => ProcessCommand::new("systemctl")
            .args(["reload", svc])
            .env("PATH", crate::resources::SECURE_PATH)
            .output(),
        NotifyTarget::DockerRestart(path) => ProcessCommand::new("docker")
            .args([
                "compose",
                "-f",
                &format!("{path}/docker-compose.yml"),
                "restart",
            ])
            .env("PATH", crate::resources::SECURE_PATH)
            .output(),
        NotifyTarget::DockerUp(path) => ProcessCommand::new("docker")
            .args([
                "compose",
                "-f",
                &format!("{path}/docker-compose.yml"),
                "up",
                "-d",
            ])
            .env("PATH", crate::resources::SECURE_PATH)
            .output(),
        NotifyTarget::Unknown(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("unknown notify target: {target}"),
        )),
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

    // Validate docker paths are absolute
    match parse_notify_target(target) {
        NotifyTarget::DockerRestart(path) | NotifyTarget::DockerUp(path) => {
            if let Err(e) = validate_docker_path(path) {
                return ResourceResult {
                    resource_type: resource_type.into(),
                    name: description,
                    status: ResourceStatus::Failed,
                    diff: None,
                    from: None,
                    to: None,
                    output: None,
                    error: Some(e),
                };
            }
        }
        _ => {}
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

/// Execute a bundle: run all resources in DAG order, dispatch handlers and shorthand
/// notify targets after the main pass, and return a summary.
///
/// Returns `Err` only when the normal-resource DAG cannot be resolved (dependency
/// cycle or unknown dependency). All other failures are recorded inside the summary.
pub fn execute_bundle(bundle: Bundle, dry_run: bool) -> Result<RunSummary, Error> {
    // Partition resources into normal vs handler
    let (normal_resources, handler_resources): (Vec<ResolvedResource>, Vec<ResolvedResource>) =
        bundle.resources.into_iter().partition(|r| !r.handler);

    // Build set of handler FQNs for notify classification
    let handler_fqns: HashSet<String> = handler_resources.iter().map(|r| r.fqn()).collect();

    // Resolve execution order for normal resources only
    let layers = match dag::resolve_order(&normal_resources) {
        Ok(l) => l,
        Err(e) => {
            return Err(e);
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

            // In dry-run mode, flag resources with unresolved register values
            if dry_run && has_unresolved_registers(&interpolated) {
                results.push(ResourceResult {
                    resource_type: resource.resource_type.clone(),
                    name: resource.name.clone(),
                    status: ResourceStatus::Changed,
                    diff: Some("register values not available in dry-run".into()),
                    from: None,
                    to: None,
                    output: None,
                    error: None,
                });
                continue;
            }

            let result = resources::execute_resource(&interpolated, dry_run, false);

            // Capture register output before redaction so downstream interpolation
            // still resolves even when the resource is marked sensitive.
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

            let result = resources::redact_result(result, resource.sensitive);
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
                        let result = resources::redact_result(result, resource.sensitive);
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

    Ok(RunSummary::from_results(&bundle.host, results))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_bundle(host: &str, resources: Vec<ResolvedResource>) -> Bundle {
        Bundle {
            host: host.to_string(),
            resources,
            facts: HashMap::new(),
        }
    }

    fn make_bundle_with_facts(
        host: &str,
        resources: Vec<ResolvedResource>,
        facts: HashMap<String, String>,
    ) -> Bundle {
        Bundle {
            host: host.to_string(),
            resources,
            facts,
        }
    }

    /// Build a ResolvedResource for a `file` type with the given path and content.
    /// File resources in dry-run always return Changed (file does not exist yet).
    fn file_resource(name: &str, path: &str) -> ResolvedResource {
        let mut props = HashMap::new();
        props.insert("path".into(), toml::Value::String(path.into()));
        props.insert("content".into(), toml::Value::String("test content".into()));
        ResolvedResource {
            resource_type: "file".into(),
            name: name.into(),
            props,
            after: vec![],
            notify: vec![],
            when: None,
            handler: false,
            register: None,
            sensitive: false,
        }
    }

    /// Build a `cmd` resource with `command` but NO guard: fails validation hermetically
    /// at cmd.rs:19-24 before any shell process is spawned.
    fn unguarded_cmd_resource(name: &str) -> ResolvedResource {
        let mut props = HashMap::new();
        props.insert("command".into(), toml::Value::String("echo fail".into()));
        ResolvedResource {
            resource_type: "cmd".into(),
            name: name.into(),
            props,
            after: vec![],
            notify: vec![],
            when: None,
            handler: false,
            register: None,
            sensitive: false,
        }
    }

    // --- execute_bundle tests ---

    /// Independent resources all run; summary counts are correct.
    #[test]
    fn execute_bundle_runs_independent_resources() {
        // Two file resources with no dependencies. In dry-run, file always
        // returns Changed (target path does not exist on the test machine).
        let r1 = file_resource("conf-a", "/nonexistent/path/a.conf");
        let r2 = file_resource("conf-b", "/nonexistent/path/b.conf");
        let bundle = make_bundle("test-host", vec![r1, r2]);

        let summary = execute_bundle(bundle, true).unwrap();

        assert_eq!(summary.host, "test-host");
        assert_eq!(summary.resources.len(), 2);
        // Both ran and produced a result (Changed in dry-run for non-existing files)
        assert_eq!(summary.summary.failed, 0);
        assert_eq!(summary.summary.skipped, 0);
    }

    /// A resource with a false `when` condition is Skipped; its error field
    /// records the expression that was not met.
    #[test]
    fn execute_bundle_skips_when_condition_false() {
        let mut r = file_resource("gated", "/nonexistent/path/gated.conf");
        r.when = Some("fact.os == 'nope'".into());

        let mut facts = HashMap::new();
        facts.insert("fact.os".into(), "linux".into());
        let bundle = make_bundle_with_facts("test-host", vec![r], facts);

        let summary = execute_bundle(bundle, true).unwrap();

        assert_eq!(summary.resources.len(), 1);
        let result = &summary.resources[0];
        assert_eq!(result.status, ResourceStatus::Skipped);
        assert_eq!(result.error.as_deref(), Some("when: fact.os == 'nope'"));
        assert_eq!(summary.summary.skipped, 1);
        assert_eq!(summary.summary.failed, 0);
    }

    /// When resource A fails, resource B (with `after = ["type.A"]`) is Skipped
    /// with error "dependency failed", and B's own FQN is also added to failed_fqns
    /// so further transitive dependents are also skipped.
    #[test]
    fn execute_bundle_skips_dependents_of_failed_resource() {
        // A: unguarded cmd -> hermetic Failed (no shell spawned)
        let mut a = unguarded_cmd_resource("a");
        a.name = "a".into();

        // B depends on A
        let mut b = file_resource("b", "/nonexistent/b.conf");
        b.after = vec!["cmd.a".into()];

        // C depends on B (transitive)
        let mut c = file_resource("c", "/nonexistent/c.conf");
        c.after = vec!["file.b".into()];

        let bundle = make_bundle("test-host", vec![a, b, c]);
        let summary = execute_bundle(bundle, true).unwrap();

        assert_eq!(summary.resources.len(), 3);

        let a_res = summary.resources.iter().find(|r| r.name == "a").unwrap();
        assert_eq!(a_res.status, ResourceStatus::Failed);

        let b_res = summary.resources.iter().find(|r| r.name == "b").unwrap();
        assert_eq!(b_res.status, ResourceStatus::Skipped);
        assert_eq!(b_res.error.as_deref(), Some("dependency failed"));

        let c_res = summary.resources.iter().find(|r| r.name == "c").unwrap();
        assert_eq!(c_res.status, ResourceStatus::Skipped);
        assert_eq!(c_res.error.as_deref(), Some("dependency failed"));

        assert_eq!(summary.summary.failed, 1);
        assert_eq!(summary.summary.skipped, 2);
    }

    /// Two resources that both notify the same shorthand target result in the
    /// shorthand being executed exactly once (dedup via shorthand_seen set).
    /// In dry-run, execute_notify_shorthand returns Changed before spawning any
    /// process, so this test is hermetic.
    #[test]
    fn execute_bundle_dedups_shorthand_notifications() {
        // Two file resources, both notifying "restart:nginx"
        let mut r1 = file_resource("conf-1", "/nonexistent/path/1.conf");
        r1.notify = vec!["restart:nginx".into()];

        let mut r2 = file_resource("conf-2", "/nonexistent/path/2.conf");
        r2.notify = vec!["restart:nginx".into()];

        let bundle = make_bundle("test-host", vec![r1, r2]);
        let summary = execute_bundle(bundle, true).unwrap();

        // 2 file resources + 1 shorthand notify (deduped from 2)
        assert_eq!(summary.resources.len(), 3);

        let notify_results: Vec<_> = summary
            .resources
            .iter()
            .filter(|r| r.resource_type == "service")
            .collect();
        assert_eq!(
            notify_results.len(),
            1,
            "shorthand notify must run exactly once, got: {notify_results:?}"
        );
        assert_eq!(notify_results[0].status, ResourceStatus::Changed);
        assert_eq!(notify_results[0].name, "nginx (restart)");
    }

    /// A handler resource (handler=true) runs only when a changed resource
    /// notifies its FQN. If nothing notifies it, it does not appear in results.
    #[test]
    fn execute_bundle_runs_handler_only_when_notified() {
        // Handler resource: a file handler (file resources bypass guard in handler mode)
        let handler_path = "/nonexistent/handler-output.txt";
        let mut handler = file_resource("nginx-reload", handler_path);
        handler.handler = true;

        // Resource that notifies the handler by FQN
        let mut notifier = file_resource("conf", "/nonexistent/path/conf.conf");
        notifier.notify = vec!["file.nginx-reload".into()];

        // Another resource that does NOT notify the handler
        let non_notifier = file_resource("other", "/nonexistent/path/other.conf");

        let bundle = make_bundle(
            "test-host",
            vec![notifier.clone(), non_notifier.clone(), handler],
        );
        let summary = execute_bundle(bundle, true).unwrap();

        // notifier + non_notifier + 1 handler execution = 3
        assert_eq!(summary.resources.len(), 3);

        let handler_results: Vec<_> = summary
            .resources
            .iter()
            .filter(|r| r.name.contains("nginx-reload"))
            .collect();
        assert_eq!(
            handler_results.len(),
            1,
            "handler must appear exactly once in results"
        );
        // Handler results have " (handler)" appended to name
        assert!(handler_results[0].name.ends_with("(handler)"));

        // Verify handler is not present when nothing notifies it
        let mut solo = file_resource("solo-handler", "/nonexistent/solo.txt");
        solo.handler = true;
        let bundle2 = make_bundle("test-host", vec![non_notifier.clone(), solo]);
        let summary2 = execute_bundle(bundle2, true).unwrap();
        // Only the non-notifier ran; handler was never triggered
        assert_eq!(summary2.resources.len(), 1);
        assert!(
            summary2
                .resources
                .iter()
                .all(|r| !r.name.contains("solo-handler")),
            "un-notified handler must not appear in results"
        );
    }
}
