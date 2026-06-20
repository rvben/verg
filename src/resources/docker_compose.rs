use std::path::Path;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, run_checked, run_cmd};

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
                run_checked(
                    "docker",
                    &["compose", "-f", &compose_path, "pull", "-q"],
                    "docker compose pull",
                )?;
            }

            // Start/restart the stack
            run_checked(
                "docker",
                &[
                    "compose",
                    "-f",
                    &compose_path,
                    "up",
                    "-d",
                    "--remove-orphans",
                ],
                "docker compose up",
            )?;
            changes.push("started".to_string());
        }
    }

    Ok(ResourceResult::from_changes(
        "docker_compose",
        resource.name.clone(),
        &changes,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn resource(props: HashMap<String, toml::Value>) -> ResolvedResource {
        ResolvedResource {
            resource_type: "docker_compose".into(),
            name: "t".into(),
            props,
            after: vec![],
            notify: vec![],
            when: None,
            handler: false,
            register: None,
            sensitive: false,
        }
    }

    #[test]
    fn missing_project_dir_is_an_error() {
        let err = execute(&resource(HashMap::new()), true).unwrap_err();
        assert!(
            err.to_string().contains("requires 'project_dir'"),
            "got: {err}"
        );
    }
}

fn stop(compose_path: &str, name: &str, dry_run: bool) -> Result<ResourceResult, Error> {
    // Check if anything is running
    let ps_output = run_cmd("docker", &["compose", "-f", compose_path, "ps", "-q"])?;
    let is_running =
        ps_output.status.success() && !String::from_utf8_lossy(&ps_output.stdout).trim().is_empty();

    if !is_running {
        return Ok(ResourceResult::ok("docker_compose", name.to_string()));
    }

    if dry_run {
        return Ok(ResourceResult::changed(
            "docker_compose",
            name.to_string(),
            "would stop",
        ));
    }

    run_checked(
        "docker",
        &["compose", "-f", compose_path, "down"],
        "docker compose down",
    )?;

    Ok(ResourceResult::changed(
        "docker_compose",
        name.to_string(),
        "stopped",
    ))
}
