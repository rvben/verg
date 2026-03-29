use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, ResourceStatus, run_cmd};

/// Manages directories with ownership and permissions.
///
/// Properties:
///   path     - Directory path
///   owner    - Owner (username or UID)
///   group    - Group (groupname or GID)
///   mode     - Permissions (octal, e.g. "0755")
///   recurse  - Apply ownership recursively (default: false)
///   state    - "present" or "absent" (default: "present")
pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let path = resource
        .props
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Resource("directory resource requires 'path'".into()))?;

    let state = resource
        .props
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("present");

    let target = Path::new(path);

    if state == "absent" {
        if !target.exists() {
            return Ok(ResourceResult {
                resource_type: "directory".into(),
                name: resource.name.clone(),
                status: ResourceStatus::Ok,
                diff: None,
                from: None,
                to: None,
                error: None,
                output: None,
            });
        }
        if dry_run {
            return Ok(ResourceResult {
                resource_type: "directory".into(),
                name: resource.name.clone(),
                status: ResourceStatus::Changed,
                diff: Some(format!("would remove {path}")),
                from: None,
                to: None,
                error: None,
                output: None,
            });
        }
        std::fs::remove_dir_all(target)
            .map_err(|e| Error::Resource(format!("failed to remove {path}: {e}")))?;
        return Ok(ResourceResult {
            resource_type: "directory".into(),
            name: resource.name.clone(),
            status: ResourceStatus::Changed,
            diff: Some(format!("removed {path}")),
            from: None,
            to: None,
            error: None,
            output: None,
        });
    }

    let mut changes = Vec::new();

    // Create directory if missing
    if !target.exists() {
        changes.push(format!("create {path}"));
        if !dry_run {
            std::fs::create_dir_all(target)
                .map_err(|e| Error::Resource(format!("failed to create {path}: {e}")))?;
        }
    }

    // Check and set mode
    if let Some(mode_str) = resource.props.get("mode").and_then(|v| v.as_str()) {
        let desired_mode = u32::from_str_radix(mode_str, 8)
            .map_err(|_| Error::Resource(format!("invalid mode: {mode_str}")))?;
        if target.exists() {
            let current_mode = std::fs::metadata(target)
                .map_err(|e| Error::Resource(format!("failed to stat {path}: {e}")))?
                .permissions()
                .mode()
                & 0o7777;
            if current_mode != desired_mode {
                changes.push(format!("mode {current_mode:04o} → {desired_mode:04o}"));
                if !dry_run {
                    std::fs::set_permissions(target, std::fs::Permissions::from_mode(desired_mode))
                        .map_err(|e| Error::Resource(format!("failed to chmod {path}: {e}")))?;
                }
            }
        }
    }

    // Check and set ownership
    let owner = resource.props.get("owner").and_then(|v| v.as_str());
    let group = resource.props.get("group").and_then(|v| v.as_str());
    let recurse = resource
        .props
        .get("recurse")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    if let Some(desired_owner) = owner {
        if target.exists() {
            let current_uid = std::fs::metadata(target)
                .map_err(|e| Error::Resource(format!("failed to stat {path}: {e}")))?
                .uid();

            // Resolve desired owner to UID for comparison
            let desired_uid = resolve_uid(desired_owner);
            let needs_change = match desired_uid {
                Some(uid) => current_uid != uid,
                None => {
                    // Compare by name via ls
                    let output = run_cmd("ls", &["-ld", path])?;
                    let ls_line = String::from_utf8_lossy(&output.stdout);
                    let current_owner = ls_line.split_whitespace().nth(2).unwrap_or("");
                    current_owner != desired_owner
                }
            };

            if needs_change {
                let chown_arg = match group {
                    Some(g) => format!("{desired_owner}:{g}"),
                    None => desired_owner.to_string(),
                };
                changes.push(format!("owner → {chown_arg}"));
                if !dry_run {
                    let mut args = vec![];
                    if recurse {
                        args.push("-R");
                    }
                    args.push(&chown_arg);
                    args.push(path);
                    let output = run_cmd("chown", &args)?;
                    if !output.status.success() {
                        let stderr = String::from_utf8_lossy(&output.stderr);
                        return Err(Error::Resource(format!("chown failed: {stderr}")));
                    }
                }
            }
        }
    } else if let Some(desired_group) = group
        && target.exists()
    {
        let output = run_cmd("ls", &["-ld", path])?;
        let ls_line = String::from_utf8_lossy(&output.stdout);
        let current_group = ls_line.split_whitespace().nth(3).unwrap_or("");
        if current_group != desired_group {
            let chgrp_target = format!(":{desired_group}");
            changes.push(format!("group → {desired_group}"));
            if !dry_run {
                let mut args = vec![];
                if recurse {
                    args.push("-R");
                }
                args.push(&chgrp_target);
                args.push(path);
                let output = run_cmd("chown", &args)?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Error::Resource(format!("chgrp failed: {stderr}")));
                }
            }
        }
    }

    Ok(ResourceResult {
        resource_type: "directory".into(),
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

/// Try to parse a string as a numeric UID.
fn resolve_uid(owner: &str) -> Option<u32> {
    owner.parse::<u32>().ok()
}
