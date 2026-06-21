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
    fn dry_run_with_hostname_prop_does_not_error_on_prop_validation() {
        // The prop is present: validation passes. Whether the system call fires
        // depends on the current hostname vs the desired one. In dry_run mode,
        // no hostnamectl is invoked, so this completes without error regardless
        // of the runtime environment.
        let mut props = HashMap::new();
        props.insert(
            "hostname".into(),
            toml::Value::String("example-host".into()),
        );
        let resource = make_resource(props);
        // In dry-run mode hostnamectl is never called, so this must not error
        // on the prop-validation side.
        let result = execute(&resource, true);
        assert!(
            result.is_ok(),
            "dry-run with valid prop must not return an error: {:?}",
            result.unwrap_err()
        );
    }

    #[test]
    fn no_change_when_current_equals_desired() {
        // We can only test the no-change path when /etc/hostname happens to
        // match. Instead, we verify the shape of the Ok result when no changes
        // are produced: from_changes returns Ok status with no diff.
        let changes: Vec<String> = vec![];
        let r = ResourceResult::from_changes("hostname", "test", &changes);
        assert_eq!(r.status, crate::resources::ResourceStatus::Ok);
        assert!(r.diff.is_none());
    }

    #[test]
    fn change_entry_format() {
        // Verify the change description format used in the diff.
        let changes = vec!["hostname old-host -> new-host".to_string()];
        let r = ResourceResult::from_changes("hostname", "test", &changes);
        assert_eq!(r.status, crate::resources::ResourceStatus::Changed);
        assert_eq!(
            r.diff.as_deref(),
            Some("hostname old-host -> new-host"),
            "diff must reflect the arrow-formatted change"
        );
    }
}
