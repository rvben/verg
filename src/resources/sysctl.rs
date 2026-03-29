use crate::error::Error;

use super::{ResolvedResource, ResourceResult, ResourceStatus, run_cmd};

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

        // Check if the entry already exists with correct value
        let has_correct_entry = current_conf.lines().any(|line| {
            let trimmed = line.trim();
            !trimmed.starts_with('#') && trimmed.replace(' ', "") == format!("{key}={desired}")
        });

        if !has_correct_entry {
            changes.push(format!("persist {key} in {conf_path}"));
            if !dry_run {
                // Keep existing entries for other keys, replace or append this key
                let mut lines: Vec<String> = current_conf
                    .lines()
                    .filter(|l| {
                        let trimmed = l.trim();
                        trimmed.starts_with('#') || !trimmed.starts_with(key)
                    })
                    .map(String::from)
                    .collect();
                lines.push(format!("{key} = {desired}"));
                let new_content = lines.join("\n") + "\n";
                std::fs::write(conf_path, new_content)
                    .map_err(|e| Error::Resource(format!("failed to write {conf_path}: {e}")))?;
            }
        }
    }

    Ok(ResourceResult {
        resource_type: "sysctl".into(),
        name: resource.name.clone(),
        status: if changes.is_empty() {
            ResourceStatus::Ok
        } else {
            ResourceStatus::Changed
        },
        diff: if changes.is_empty() {
            None
        } else {
            Some(changes.join(", "))
        },
        from: None,
        to: None,
        error: None,
        output: None,
    })
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
}
