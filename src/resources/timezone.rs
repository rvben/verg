use crate::error::Error;

use super::{ResolvedResource, ResourceResult, run_checked, run_cmd};

pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let desired = resource.prop_str_required("timezone")?;

    let current = read_current_timezone();

    let mut changes = Vec::new();

    if current != desired {
        changes.push(format!("timezone {current} -> {desired}"));
        if !dry_run {
            run_checked("timedatectl", &["set-timezone", desired], "set-timezone")?;
        }
    }

    Ok(ResourceResult::from_changes(
        "timezone",
        resource.name.clone(),
        &changes,
    ))
}

/// Read the current timezone via timedatectl. Returns an empty string on any
/// command failure so the resource drifts and converges on next apply.
fn read_current_timezone() -> String {
    run_cmd("timedatectl", &["show", "-p", "Timezone", "--value"])
        .map(|output| {
            if output.status.success() {
                String::from_utf8_lossy(&output.stdout).trim().to_string()
            } else {
                String::new()
            }
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_resource(props: HashMap<String, toml::Value>) -> ResolvedResource {
        crate::resources::test_resource("timezone", "test", props)
    }

    #[test]
    fn missing_timezone_prop_returns_error() {
        let props = HashMap::new();
        let resource = make_resource(props);
        let result = execute(&resource, true);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("requires 'timezone'"),
            "error must mention the missing 'timezone' prop"
        );
    }

    #[test]
    fn dry_run_with_timezone_prop_does_not_error_on_prop_validation() {
        // The prop is present: validation passes. In dry-run mode, no
        // timedatectl set-timezone is invoked for the apply step, so this
        // completes without an apply error regardless of the runtime environment.
        // (read_current_timezone may return empty if timedatectl is unavailable,
        // which counts as drift and still completes cleanly in dry_run.)
        let mut props = HashMap::new();
        props.insert(
            "timezone".into(),
            toml::Value::String("Europe/Amsterdam".into()),
        );
        let resource = make_resource(props);
        let result = execute(&resource, true);
        assert!(
            result.is_ok(),
            "dry-run with valid prop must not return an error: {:?}",
            result.unwrap_err()
        );
    }

    #[test]
    fn no_change_when_current_equals_desired() {
        // Verify the shape of the Ok result when no changes are produced.
        let changes: Vec<String> = vec![];
        let r = ResourceResult::from_changes("timezone", "test", &changes);
        assert_eq!(r.status, crate::resources::ResourceStatus::Ok);
        assert!(r.diff.is_none());
    }

    #[test]
    fn change_entry_format() {
        // Verify the change description format used in the diff.
        let changes = vec!["timezone UTC -> Europe/Amsterdam".to_string()];
        let r = ResourceResult::from_changes("timezone", "test", &changes);
        assert_eq!(r.status, crate::resources::ResourceStatus::Changed);
        assert_eq!(
            r.diff.as_deref(),
            Some("timezone UTC -> Europe/Amsterdam"),
            "diff must reflect the arrow-formatted change"
        );
    }
}
