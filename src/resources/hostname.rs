use crate::error::Error;

use super::{ResolvedResource, ResourceResult, run_checked};

pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let desired = resource.prop_str_required("hostname")?;

    let current = read_static_hostname();

    let mut changes = Vec::new();

    if current != desired {
        changes.push(format!("hostname {current} -> {desired}"));
        if !dry_run {
            run_checked("hostnamectl", &["set-hostname", desired], "set-hostname")?;
        }
    }

    Ok(ResourceResult::from_changes(
        "hostname",
        resource.name.clone(),
        &changes,
    ))
}

/// Read the static hostname from /etc/hostname, trimming whitespace.
/// Any read failure (missing file, permission error, etc.) returns an empty
/// string, which causes the resource to drift and converge on next apply.
fn read_static_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_resource(props: HashMap<String, toml::Value>) -> ResolvedResource {
        crate::resources::test_resource("hostname", "test", props)
    }

    #[test]
    fn missing_hostname_prop_returns_error() {
        let props = HashMap::new();
        let resource = make_resource(props);
        let result = execute(&resource, true);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("requires 'hostname'"),
            "error must mention the missing 'hostname' prop"
        );
    }

    #[test]
    fn dry_run_reports_change_when_hostname_differs_and_does_not_mutate() {
        // A desired hostname that cannot match the test host's actual
        // /etc/hostname drives the drift path through the real executor. dry-run
        // must report Changed (with the arrow-formatted diff) and never invoke
        // hostnamectl, regardless of the runtime environment.
        let mut props = HashMap::new();
        props.insert(
            "hostname".into(),
            toml::Value::String("verg-test-sentinel-hostname".into()),
        );
        let resource = make_resource(props);
        let result = execute(&resource, true).unwrap();
        assert_eq!(
            result.status,
            crate::resources::ResourceStatus::Changed,
            "a differing hostname must report Changed via the real executor"
        );
        assert!(
            result
                .diff
                .as_deref()
                .unwrap_or("")
                .contains("-> verg-test-sentinel-hostname"),
            "diff must show the arrow-formatted hostname change: {:?}",
            result.diff
        );
    }
}
