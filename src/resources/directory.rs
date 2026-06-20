use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, run_checked, run_cmd};

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
    let path = resource.prop_str_required("path")?;

    let state = resource.prop_str_or("state", "present");

    let target = Path::new(path);

    if state == "absent" {
        if !target.exists() {
            return Ok(ResourceResult::ok("directory", resource.name.clone()));
        }
        if dry_run {
            return Ok(ResourceResult::changed(
                "directory",
                resource.name.clone(),
                format!("would remove {path}"),
            ));
        }
        std::fs::remove_dir_all(target)
            .map_err(|e| Error::Resource(format!("failed to remove {path}: {e}")))?;
        return Ok(ResourceResult::changed(
            "directory",
            resource.name.clone(),
            format!("removed {path}"),
        ));
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
    let recurse = resource.prop_bool_or("recurse", false);

    if (owner.is_some() || group.is_some()) && target.exists() {
        // MetadataExt (uid/gid) is already imported at the top of this file.
        let meta = std::fs::metadata(target)
            .map_err(|e| Error::Resource(format!("failed to stat {path}: {e}")))?;

        // Owner matches: compare by UID when numeric, else by name via `ls`.
        let owner_matches = match owner {
            Some(o) => match resolve_uid(o) {
                Some(uid) => meta.uid() == uid,
                None => {
                    let out = run_cmd("ls", &["-ld", path])?;
                    let line = String::from_utf8_lossy(&out.stdout);
                    line.split_whitespace().nth(2).unwrap_or("") == o
                }
            },
            None => true,
        };

        // Group matches: compare by GID when numeric, else by name via `ls`.
        let group_matches = match group {
            Some(g) => match g.parse::<u32>().ok() {
                Some(gid) => meta.gid() == gid,
                None => {
                    let out = run_cmd("ls", &["-ld", path])?;
                    let line = String::from_utf8_lossy(&out.stdout);
                    line.split_whitespace().nth(3).unwrap_or("") == g
                }
            },
            None => true,
        };

        if let Some(arg) = ownership_action(owner, owner_matches, group, group_matches) {
            changes.push(format!("owner/group -> {arg}"));
            if !dry_run {
                let mut args = vec![];
                if recurse {
                    args.push("-R");
                }
                args.push(arg.as_str());
                args.push(path);
                run_checked("chown", &args, "chown")?;
            }
        }
    }

    Ok(ResourceResult::from_changes(
        "directory",
        resource.name.clone(),
        &changes,
    ))
}

/// Try to parse a string as a numeric UID.
fn resolve_uid(owner: &str) -> Option<u32> {
    owner.parse::<u32>().ok()
}

/// Build the `chown` target argument from optional owner and group.
fn build_chown_arg(owner: Option<&str>, group: Option<&str>) -> Option<String> {
    match (owner, group) {
        (Some(o), Some(g)) => Some(format!("{o}:{g}")),
        (Some(o), None) => Some(o.to_string()),
        (None, Some(g)) => Some(format!(":{g}")),
        (None, None) => None,
    }
}

/// Decide the `chown` arg given whether owner/group are specified and whether
/// each already matches. Returns the arg when owner OR group drifts, else None.
/// Group drift triggers a chown even when owner already matches.
fn ownership_action(
    owner: Option<&str>,
    owner_matches: bool,
    group: Option<&str>,
    group_matches: bool,
) -> Option<String> {
    let owner_drift = owner.is_some() && !owner_matches;
    let group_drift = group.is_some() && !group_matches;
    if owner_drift || group_drift {
        build_chown_arg(owner, group)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chown_arg_combines_owner_and_group() {
        assert_eq!(
            build_chown_arg(Some("deploy"), Some("docker")).as_deref(),
            Some("deploy:docker")
        );
        assert_eq!(
            build_chown_arg(Some("deploy"), None).as_deref(),
            Some("deploy")
        );
        assert_eq!(
            build_chown_arg(None, Some("docker")).as_deref(),
            Some(":docker")
        );
        assert_eq!(build_chown_arg(None, None), None);
    }

    #[test]
    fn group_drift_triggers_chown_even_when_owner_matches() {
        // The W2-6 bug: owner already correct, group wrong -> must still chown.
        assert_eq!(
            ownership_action(Some("deploy"), true, Some("docker"), false).as_deref(),
            Some("deploy:docker")
        );
    }

    #[test]
    fn no_drift_means_no_chown() {
        assert_eq!(
            ownership_action(Some("deploy"), true, Some("docker"), true),
            None
        );
        assert_eq!(ownership_action(None, true, None, true), None);
    }

    #[test]
    fn owner_drift_alone_triggers_chown() {
        assert_eq!(
            ownership_action(Some("deploy"), false, None, true).as_deref(),
            Some("deploy")
        );
    }
}
