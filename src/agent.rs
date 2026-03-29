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
pub fn parse_notify_target(target: &str) -> NotifyTarget<'_> {
    if target == "daemon-reload" {
        NotifyTarget::DaemonReload
    } else if let Some(svc) = target.strip_prefix("restart:") {
        NotifyTarget::Restart(svc)
    } else if let Some(svc) = target.strip_prefix("reload:") {
        NotifyTarget::Reload(svc)
    } else if let Some(path) = target.strip_prefix("docker-restart:") {
        NotifyTarget::DockerRestart(path)
    } else if let Some(path) = target.strip_prefix("docker-up:") {
        NotifyTarget::DockerUp(path)
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
        }
    }

    // --- interpolate_registers ---

    #[test]
    fn interpolate_registers_replaces_matching_sentinel() {
        let resource = make_resource(&[("content", "__VERG_REG_ip__VERG_END__")]);
        let mut registers = HashMap::new();
        registers.insert("ip".into(), "10.0.0.1".into());

        let result = interpolate_registers(&resource, &registers);
        assert_eq!(result.props["content"].as_str().unwrap(), "10.0.0.1");
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
        let resource = make_resource(&[("content", "10.0.0.1")]);
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
    fn describe_notify_unknown() {
        let (rt, desc) = describe_notify("custom-action");
        assert_eq!(rt, "notify");
        assert_eq!(desc, "custom-action (notify)");
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
        assert_eq!(
            parse_notify_target("something-else"),
            NotifyTarget::Unknown("something-else")
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
