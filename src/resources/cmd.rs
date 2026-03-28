use std::path::Path;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, ResourceStatus, run_cmd};

pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let command = resource
        .props
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Resource("cmd resource requires 'command'".into()))?;

    let creates = resource.props.get("creates").and_then(|v| v.as_str());
    let unless = resource.props.get("unless").and_then(|v| v.as_str());
    let onlyif = resource.props.get("onlyif").and_then(|v| v.as_str());

    if creates.is_none() && unless.is_none() && onlyif.is_none() {
        return Err(Error::Resource(
            "cmd resource requires at least one guard: 'creates', 'unless', or 'onlyif'".into(),
        ));
    }

    if let Some(path) = creates
        && Path::new(path).exists()
    {
        return Ok(ResourceResult {
            resource_type: "cmd".into(),
            name: resource.name.clone(),
            status: ResourceStatus::Ok,
            diff: None,
            from: None,
            to: None,
            error: None,
        });
    }

    if let Some(cmd) = unless {
        let output = run_cmd("sh", &["-c", cmd])?;
        if output.status.success() {
            return Ok(ResourceResult {
                resource_type: "cmd".into(),
                name: resource.name.clone(),
                status: ResourceStatus::Ok,
                diff: None,
                from: None,
                to: None,
                error: None,
            });
        }
    }

    if let Some(cmd) = onlyif {
        let output = run_cmd("sh", &["-c", cmd])?;
        if !output.status.success() {
            return Ok(ResourceResult {
                resource_type: "cmd".into(),
                name: resource.name.clone(),
                status: ResourceStatus::Ok,
                diff: None,
                from: None,
                to: None,
                error: None,
            });
        }
    }

    if dry_run {
        return Ok(ResourceResult {
            resource_type: "cmd".into(),
            name: resource.name.clone(),
            status: ResourceStatus::Changed,
            diff: Some(format!("would run: {command}")),
            from: None,
            to: None,
            error: None,
        });
    }

    let output = run_cmd("sh", &["-c", command])?;
    if output.status.success() {
        Ok(ResourceResult {
            resource_type: "cmd".into(),
            name: resource.name.clone(),
            status: ResourceStatus::Changed,
            diff: None,
            from: None,
            to: None,
            error: None,
        })
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(Error::Resource(format!("command failed: {stderr}")))
    }
}
