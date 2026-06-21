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
    fn dry_run_reports_change_when_timezone_differs_and_does_not_mutate() {
        // A desired timezone that cannot match the host's current one drives the
        // drift path through the real executor. dry-run must report Changed (with
        // the arrow-formatted diff) and never invoke timedatectl set-timezone.
        let mut props = HashMap::new();
        props.insert(
            "timezone".into(),
            toml::Value::String("Etc/Verg-Test-Sentinel".into()),
        );
        let resource = make_resource(props);
        let result = execute(&resource, true).unwrap();
        assert_eq!(
            result.status,
            crate::resources::ResourceStatus::Changed,
            "a differing timezone must report Changed via the real executor"
        );
        assert!(
            result
                .diff
                .as_deref()
                .unwrap_or("")
                .contains("-> Etc/Verg-Test-Sentinel"),
            "diff must show the arrow-formatted timezone change: {:?}",
            result.diff
        );
    }
}
