use crate::error::Error;

use super::{ResolvedResource, ResourceResult, run_cmd};

/// Rebuild the persisted sysctl.d content: keep comments and every line whose
/// key (left of `=`) differs from `key`, then append `key = desired`.
fn rebuild_sysctl_conf(existing: &str, key: &str, desired: &str) -> String {
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                return true;
            }
            match trimmed.split_once('=') {
                Some((lhs, _)) => lhs.trim() != key,
                None => true,
            }
        })
        .map(String::from)
        .collect();
    lines.push(format!("{key} = {desired}"));
    lines.join("\n") + "\n"
}

pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let key = resource
        .props
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Resource("sysctl resource requires 'key'".into()))?;
    let desired = resource
        .props
        .get("value")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Resource("sysctl resource requires 'value'".into()))?;
    let persist = resource
        .props
        .get("persist")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // Check current value
    let output = run_cmd("sysctl", &["-n", key])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Resource(format!(
            "failed to read sysctl key '{key}': {stderr}"
        )));
    }
    let current = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let mut changes = Vec::new();

    if current != desired {
        changes.push(format!("{key}: {current} → {desired}"));
        if !dry_run {
            let set_output = run_cmd("sysctl", &["-w", &format!("{key}={desired}")])?;
            if !set_output.status.success() {
                let stderr = String::from_utf8_lossy(&set_output.stderr);
                return Err(Error::Resource(format!("sysctl -w failed: {stderr}")));
            }
        }
    }

    // Persist to /etc/sysctl.d/99-verg.conf so the setting survives reboot
    if persist {
        let conf_path = "/etc/sysctl.d/99-verg.conf";
        let current_conf = std::fs::read_to_string(conf_path).unwrap_or_default();

        // Space-tolerant: normalizes "key = val" and "key=val" to the same form for comparison.
        let has_correct_entry = current_conf.lines().any(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with('#') && trimmed.replace(' ', "") == format!("{key}={desired}")
        });

        if !has_correct_entry {
            changes.push(format!("persist {key} in {conf_path}"));
            if !dry_run {
                let new_content = rebuild_sysctl_conf(&current_conf, key, desired);
                crate::resources::atomic::write_atomic(
                    std::path::Path::new(conf_path),
                    new_content.as_bytes(),
                    None,
                )
                .map_err(|e| Error::Resource(format!("failed to write {conf_path}: {e}")))?;
            }
        }
    }

    Ok(ResourceResult::from_changes(
        "sysctl",
        resource.name.clone(),
        &changes,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_resource(props: HashMap<String, toml::Value>) -> ResolvedResource {
        ResolvedResource {
            resource_type: "sysctl".into(),
            name: "test".into(),
            props,
            after: vec![],
            notify: vec![],
            when: None,
            handler: false,
            register: None,
            sensitive: false,
        }
    }

    #[test]
    fn missing_key_returns_error() {
        let mut props = HashMap::new();
        props.insert("value".into(), toml::Value::String("1".into()));
        let resource = make_resource(props);
        let result = execute(&resource, true);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("requires 'key'"));
    }

    #[test]
    fn missing_value_returns_error() {
        let mut props = HashMap::new();
        props.insert(
            "key".into(),
            toml::Value::String("net.ipv4.ip_forward".into()),
        );
        let resource = make_resource(props);
        let result = execute(&resource, true);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("requires 'value'"));
    }

    #[test]
    fn rebuild_replaces_only_exact_key() {
        let existing = "# header\nnet.ipv4.ip_forward = 0\nnet.ipv4.ip_forward_use_pmtu = 1\n";
        let out = rebuild_sysctl_conf(existing, "net.ipv4.ip_forward", "1");
        // The prefix-sibling key must survive.
        assert!(
            out.contains("net.ipv4.ip_forward_use_pmtu = 1"),
            "sibling lost: {out}"
        );
        // The target key is set to the new value exactly once.
        assert!(
            out.contains("net.ipv4.ip_forward = 1"),
            "key not set: {out}"
        );
        assert_eq!(
            out.matches("net.ipv4.ip_forward =").count(),
            1,
            "duplicate: {out}"
        );
        // Comments are preserved.
        assert!(out.contains("# header"));
    }

    #[test]
    fn rebuild_appends_when_absent() {
        let out = rebuild_sysctl_conf("", "vm.swappiness", "10");
        assert_eq!(out, "vm.swappiness = 10\n");
    }
}
