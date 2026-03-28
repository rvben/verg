use std::path::Path;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, ResourceStatus, run_cmd};

/// Manages Docker Compose services.
///
/// Properties:
///   project_dir  - Directory on target where compose file lives
///   content      - Compose file content (inlined by bundle builder from compose_file)
///   state        - "up" or "down" (default: "up")
///   pull         - Pull images before starting (default: true)
pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let project_dir = resource
        .props
        .get("project_dir")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Resource("docker_compose resource requires 'project_dir'".into()))?;

    let state = resource
        .props
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("up");

    let pull = resource
        .props
        .get("pull")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let compose_path = format!("{project_dir}/docker-compose.yml");

    if state == "down" {
        return stop(&compose_path, &resource.name, dry_run);
    }

    let mut changes = Vec::new();

    // Ensure project directory exists
    if !Path::new(project_dir).exists() {
        changes.push(format!("create {project_dir}"));
        if !dry_run {
            std::fs::create_dir_all(project_dir)
                .map_err(|e| Error::Resource(format!("failed to create {project_dir}: {e}")))?;
        }
    }

    // Deploy compose file if content is provided
    if let Some(content) = resource.props.get("content").and_then(|v| v.as_str()) {
        let current = if Path::new(&compose_path).exists() {
            std::fs::read_to_string(&compose_path).ok()
        } else {
            None
        };

        if current.as_deref() != Some(content) {
            changes.push("compose file updated".to_string());
            if !dry_run {
                std::fs::write(&compose_path, content)
                    .map_err(|e| Error::Resource(format!("failed to write {compose_path}: {e}")))?;
            }
        }
    }

    // Deploy env file if provided
    if let Some(env_content) = resource.props.get("env_content").and_then(|v| v.as_str()) {
        let env_path = format!("{project_dir}/.env");
        let current = if Path::new(&env_path).exists() {
            std::fs::read_to_string(&env_path).ok()
        } else {
            None
        };

        if current.as_deref() != Some(env_content) {
            changes.push(".env updated".to_string());
            if !dry_run {
                std::fs::write(&env_path, env_content)
                    .map_err(|e| Error::Resource(format!("failed to write {env_path}: {e}")))?;
            }
        }
    }

    // Check if compose stack is running
    let ps_output = run_cmd("docker", &["compose", "-f", &compose_path, "ps", "-q"])?;
    let is_running =
        ps_output.status.success() && !String::from_utf8_lossy(&ps_output.stdout).trim().is_empty();

    if !is_running || !changes.is_empty() {
        if !is_running {
            changes.push("containers not running".to_string());
        }

        if dry_run {
            changes.push("would start".to_string());
        } else {
            // Pull images if requested
            if pull {
                let output = run_cmd("docker", &["compose", "-f", &compose_path, "pull", "-q"])?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Error::Resource(format!(
                        "docker compose pull failed: {stderr}"
                    )));
                }
            }

            // Start/restart the stack
            let output = run_cmd(
                "docker",
                &[
                    "compose",
                    "-f",
                    &compose_path,
                    "up",
                    "-d",
                    "--remove-orphans",
                ],
            )?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(Error::Resource(format!(
                    "docker compose up failed: {stderr}"
                )));
            }
            changes.push("started".to_string());
        }
    }

    Ok(ResourceResult {
        resource_type: "docker_compose".into(),
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
    })
}

fn stop(compose_path: &str, name: &str, dry_run: bool) -> Result<ResourceResult, Error> {
    // Check if anything is running
    let ps_output = run_cmd("docker", &["compose", "-f", compose_path, "ps", "-q"])?;
    let is_running =
        ps_output.status.success() && !String::from_utf8_lossy(&ps_output.stdout).trim().is_empty();

    if !is_running {
        return Ok(ResourceResult {
            resource_type: "docker_compose".into(),
            name: name.to_string(),
            status: ResourceStatus::Ok,
            diff: None,
            from: None,
            to: None,
            error: None,
        });
    }

    if dry_run {
        return Ok(ResourceResult {
            resource_type: "docker_compose".into(),
            name: name.to_string(),
            status: ResourceStatus::Changed,
            diff: Some("would stop".to_string()),
            from: None,
            to: None,
            error: None,
        });
    }

    let output = run_cmd("docker", &["compose", "-f", compose_path, "down"])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Resource(format!(
            "docker compose down failed: {stderr}"
        )));
    }

    Ok(ResourceResult {
        resource_type: "docker_compose".into(),
        name: name.to_string(),
        status: ResourceStatus::Changed,
        diff: Some("stopped".to_string()),
        from: None,
        to: None,
        error: None,
    })
}
