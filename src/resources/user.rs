use crate::error::Error;

use super::{ResolvedResource, ResourceResult, ResourceStatus, run_cmd};

fn user_exists(name: &str) -> Result<bool, Error> {
    let output = run_cmd("id", &[name])?;
    Ok(output.status.success())
}

pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let name = resource
        .props
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Resource("user resource requires 'name'".into()))?;

    let state = resource
        .props
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("present");

    let exists = user_exists(name)?;

    match (state, exists) {
        ("present", false) => {
            if dry_run {
                return Ok(ResourceResult {
                    resource_type: "user".into(),
                    name: resource.name.clone(),
                    status: ResourceStatus::Changed,
                    diff: Some(format!("would create user {name}")),
                    from: None,
                    to: None,
                    error: None,
                    output: None,
                });
            }
            let mut args = vec!["--system"];
            if let Some(home) = resource.props.get("home").and_then(|v| v.as_str()) {
                args.extend(["-d", home, "-m"]);
            }
            if let Some(shell) = resource.props.get("shell").and_then(|v| v.as_str()) {
                args.extend(["-s", shell]);
            }
            if let Some(groups) = resource.props.get("groups").and_then(|v| v.as_str()) {
                args.extend(["-G", groups]);
            }
            args.push(name);
            let output = run_cmd("useradd", &args)?;
            if output.status.success() {
                Ok(ResourceResult {
                    resource_type: "user".into(),
                    name: resource.name.clone(),
                    status: ResourceStatus::Changed,
                    diff: Some(format!("created user {name}")),
                    from: None,
                    to: None,
                    error: None,
                    output: None,
                })
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(Error::Resource(format!("useradd failed: {stderr}")))
            }
        }
        ("absent", true) => {
            if dry_run {
                return Ok(ResourceResult {
                    resource_type: "user".into(),
                    name: resource.name.clone(),
                    status: ResourceStatus::Changed,
                    diff: Some(format!("would remove user {name}")),
                    from: None,
                    to: None,
                    error: None,
                    output: None,
                });
            }
            let output = run_cmd("userdel", &["-r", name])?;
            if output.status.success() {
                Ok(ResourceResult {
                    resource_type: "user".into(),
                    name: resource.name.clone(),
                    status: ResourceStatus::Changed,
                    diff: Some(format!("removed user {name}")),
                    from: None,
                    to: None,
                    error: None,
                    output: None,
                })
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(Error::Resource(format!("userdel failed: {stderr}")))
            }
        }
        _ => Ok(ResourceResult {
            resource_type: "user".into(),
            name: resource.name.clone(),
            status: ResourceStatus::Ok,
            diff: None,
            from: None,
            to: None,
            error: None,
            output: None,
        }),
    }
}
