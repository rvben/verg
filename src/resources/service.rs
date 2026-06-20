use crate::error::Error;

use super::{ResolvedResource, ResourceResult, run_cmd};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enabled_states_are_enabled() {
        assert!(systemctl_reports_enabled("enabled\n"));
        assert!(systemctl_reports_enabled("enabled-runtime\n"));
    }

    #[test]
    fn non_enabled_states_are_not_enabled() {
        for s in [
            "disabled\n",
            "static\n",
            "masked\n",
            "indirect\n",
            "generated\n",
            "",
        ] {
            assert!(
                !systemctl_reports_enabled(s),
                "should not be enabled: {s:?}"
            );
        }
    }
}

fn is_active(name: &str) -> Result<bool, Error> {
    let output = run_cmd("systemctl", &["is-active", name])?;
    Ok(output.status.success())
}

/// True only when the unit is genuinely enabled. `systemctl is-enabled` exits
/// 0 for `static`/`indirect`/`alias`/`generated` too, so exit code alone
/// misclassifies units that cannot be enabled or disabled.
fn systemctl_reports_enabled(output: &str) -> bool {
    matches!(output.trim(), "enabled" | "enabled-runtime")
}

fn is_enabled(name: &str) -> Result<bool, Error> {
    let output = run_cmd("systemctl", &["is-enabled", name])?;
    Ok(systemctl_reports_enabled(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn systemctl(action: &str, name: &str) -> Result<(), Error> {
    let output = run_cmd("systemctl", &[action, name])?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(Error::Resource(format!(
            "systemctl {action} {name} failed: {stderr}"
        )))
    }
}

pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let name = resource.prop_str_required("name")?;

    let desired_state = resource.prop_str_or("state", "running");

    let desired_enabled = resource.props.get("enabled").and_then(|v| v.as_bool());

    let mut changes = Vec::new();

    let active = is_active(name)?;
    match (desired_state, active) {
        ("running", false) => {
            changes.push(format!("{name}: stopped → running"));
            if !dry_run {
                systemctl("start", name)?;
            }
        }
        ("stopped", true) => {
            changes.push(format!("{name}: running → stopped"));
            if !dry_run {
                systemctl("stop", name)?;
            }
        }
        _ => {}
    }

    if let Some(want_enabled) = desired_enabled {
        let enabled = is_enabled(name)?;
        if want_enabled && !enabled {
            changes.push(format!("{name}: disabled → enabled"));
            if !dry_run {
                systemctl("enable", name)?;
            }
        } else if !want_enabled && enabled {
            changes.push(format!("{name}: enabled → disabled"));
            if !dry_run {
                systemctl("disable", name)?;
            }
        }
    }

    Ok(ResourceResult::from_changes(
        "service",
        resource.name.clone(),
        &changes,
    ))
}
