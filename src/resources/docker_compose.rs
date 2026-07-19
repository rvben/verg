use std::path::Path;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, run_checked, run_cmd};

const START_ACTION: &str = "would start (containers not running)";
const RECONCILE_ACTION: &str = "would reconcile via docker compose up -d";

/// The dry-run action line for an `up` compose stack, given whether it is
/// running and whether its files changed. `None` when `up -d` would be a no-op
/// (a running stack whose files are unchanged), so a converged stack reports no
/// change instead of a blanket "would start".
fn planned_action(is_running: bool, files_changed: bool) -> Option<&'static str> {
    match (is_running, files_changed) {
        (false, _) => Some(START_ACTION),
        (true, true) => Some(RECONCILE_ACTION),
        (true, false) => None,
    }
}

/// Manages Docker Compose services.
///
/// Properties:
///   project_dir  - Directory on target where compose file lives
///   content      - Compose file content (inlined by bundle builder from compose_file)
///   state        - "up" or "down" (default: "up")
///   pull         - Pull images before starting (default: true)
pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let project_dir = resource.prop_str_required("project_dir")?;

    let state = resource.prop_str_or("state", "up");

    let pull = resource.prop_bool_or("pull", true);

    let compose_path = format!("{project_dir}/docker-compose.yml");

    if state == "down" {
        return stop(&compose_path, &resource.name, dry_run);
    }

    let mut changes = Vec::new();
    let mut field_changes: Vec<super::FieldChange> = Vec::new();

    // Ensure project directory exists
    if !Path::new(project_dir).exists() {
        changes.push(format!("create {project_dir}"));
        field_changes.push(super::FieldChange::create("project_dir", project_dir));
        if !dry_run {
            std::fs::create_dir_all(project_dir)
                .map_err(|e| Error::Resource(format!("failed to create {project_dir}: {e}")))?;
        }
    }

    // Deploy compose file if content is provided
    if let Some(content) = resource.props.get("content").and_then(|v| v.as_str()) {
        let current = crate::resources::read_current(Path::new(&compose_path))?;

        if current.as_deref() != Some(content) {
            changes.push("compose file updated".to_string());
            field_changes.push(super::FieldChange {
                field: "compose_file".to_string(),
                action: if current.is_none() {
                    super::ChangeAction::Create
                } else {
                    super::ChangeAction::Update
                },
                from: current.as_deref().map(crate::resources::content_digest),
                to: Some(crate::resources::content_digest(content)),
            });
            if !dry_run {
                crate::resources::atomic::write_atomic(
                    Path::new(&compose_path),
                    content.as_bytes(),
                    None,
                )
                .map_err(|e| Error::Resource(format!("failed to write {compose_path}: {e}")))?;
            }
        }
    }

    // Deploy env file if provided
    if let Some(env_content) = resource.props.get("env_content").and_then(|v| v.as_str()) {
        let env_path = format!("{project_dir}/.env");
        let current = crate::resources::read_current(Path::new(&env_path))?;

        if current.as_deref() != Some(env_content) {
            changes.push(".env updated".to_string());
            field_changes.push(super::FieldChange {
                field: "env_file".to_string(),
                action: if current.is_none() {
                    super::ChangeAction::Create
                } else {
                    super::ChangeAction::Update
                },
                from: current.as_deref().map(crate::resources::content_digest),
                to: Some(crate::resources::content_digest(env_content)),
            });
            if !dry_run {
                crate::resources::atomic::write_atomic(
                    Path::new(&env_path),
                    env_content.as_bytes(),
                    None,
                )
                .map_err(|e| Error::Resource(format!("failed to write {env_path}: {e}")))?;
            }
        }
    }

    // Check if compose stack is running
    let ps_output = run_cmd("docker", &["compose", "-f", &compose_path, "ps", "-q"])?;
    let is_running =
        ps_output.status.success() && !String::from_utf8_lossy(&ps_output.stdout).trim().is_empty();

    let files_changed = !changes.is_empty();

    // A stack transition (start when down, reconcile when a running stack's files
    // changed) is a change even when no file was rewritten; record it structurally
    // so machine consumers see it, not just the human diff.
    let stack_change = |is_running: bool| super::FieldChange {
        field: "stack".to_string(),
        action: if is_running {
            super::ChangeAction::Update
        } else {
            super::ChangeAction::Create
        },
        from: None,
        to: None,
    };

    if dry_run {
        // Report precisely what `up -d` would do: start a down stack, reconcile
        // a running one whose files changed, or nothing for a converged stack.
        if let Some(action) = planned_action(is_running, files_changed) {
            changes.push(action.to_string());
            field_changes.push(stack_change(is_running));
        }
    } else if !is_running || files_changed {
        field_changes.push(stack_change(is_running));
        if !is_running {
            changes.push("containers not running".to_string());
        }
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

    Ok(
        ResourceResult::from_changes("docker_compose", resource.name.clone(), &changes)
            .with_changes(field_changes),
    )
}

fn stop(compose_path: &str, name: &str, dry_run: bool) -> Result<ResourceResult, Error> {
    // Check if anything is running
    let ps_output = run_cmd("docker", &["compose", "-f", compose_path, "ps", "-q"])?;
    let is_running =
        ps_output.status.success() && !String::from_utf8_lossy(&ps_output.stdout).trim().is_empty();

    if !is_running {
        return Ok(ResourceResult::ok("docker_compose", name.to_string()));
    }

    let stop_change = vec![super::FieldChange::delete("stack", name)];
    if dry_run {
        return Ok(
            ResourceResult::changed("docker_compose", name.to_string(), "would stop")
                .with_changes(stop_change),
        );
    }

    run_checked(
        "docker",
        &["compose", "-f", compose_path, "down"],
        "docker compose down",
    )?;

    Ok(
        ResourceResult::changed("docker_compose", name.to_string(), "stopped")
            .with_changes(stop_change),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn resource(props: HashMap<String, toml::Value>) -> ResolvedResource {
        crate::resources::test_resource("docker_compose", "t", props)
    }

    #[test]
    fn missing_project_dir_is_an_error() {
        let err = execute(&resource(HashMap::new()), true).unwrap_err();
        assert!(
            err.to_string().contains("requires 'project_dir'"),
            "got: {err}"
        );
    }

    #[test]
    fn planned_action_is_precise() {
        // Down -> start; running + file change -> reconcile (not "start");
        // running + no change -> nothing (a healthy converged stack is a no-op).
        assert_eq!(planned_action(false, false), Some(START_ACTION));
        assert_eq!(planned_action(false, true), Some(START_ACTION));
        assert_eq!(planned_action(true, true), Some(RECONCILE_ACTION));
        assert_eq!(planned_action(true, false), None);
    }
}
