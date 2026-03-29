use crate::error::Error;

use super::{ResolvedResource, ResourceResult, ResourceStatus, run_cmd};

fn is_active(name: &str) -> Result<bool, Error> {
    let output = run_cmd("systemctl", &["is-active", name])?;
    Ok(output.status.success())
}

fn is_enabled(name: &str) -> Result<bool, Error> {
    let output = run_cmd("systemctl", &["is-enabled", name])?;
    Ok(output.status.success())
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
    let name = resource
        .props
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Resource("service resource requires 'name'".into()))?;

    let desired_state = resource
        .props
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("running");

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

    Ok(ResourceResult {
        resource_type: "service".into(),
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
