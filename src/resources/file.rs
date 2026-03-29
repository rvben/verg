use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, ResourceStatus, run_cmd};

pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let path = resource
        .props
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Resource("file resource requires 'path'".into()))?;

    let target = Path::new(path);
    let mut changes = Vec::new();

    let desired_content =
        if let Some(content) = resource.props.get("content").and_then(|v| v.as_str()) {
            Some(content.to_string())
        } else if let Some(source) = resource.props.get("source").and_then(|v| v.as_str()) {
            Some(
                std::fs::read_to_string(source)
                    .map_err(|e| Error::Resource(format!("failed to read source {source}: {e}")))?,
            )
        } else {
            None
        };

    if let Some(desired) = &desired_content {
        let current = if target.exists() {
            std::fs::read_to_string(target).ok()
        } else {
            None
        };
        if current.as_deref() != Some(desired.as_str()) {
            changes.push("content".to_string());
            if !dry_run {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| Error::Resource(format!("failed to create dir: {e}")))?;
                }
                std::fs::write(target, desired)
                    .map_err(|e| Error::Resource(format!("failed to write {path}: {e}")))?;
            }
        }
    }

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

    if let Some(owner) = resource.props.get("owner").and_then(|v| v.as_str())
        && target.exists()
    {
        // Use ls -ld for portable owner detection (works on Linux and macOS)
        let ls_output = run_cmd("ls", &["-ld", path])?;
        let ls_line = String::from_utf8_lossy(&ls_output.stdout);
        let current_owner = ls_line.split_whitespace().nth(2).unwrap_or("");
        if current_owner != owner {
            changes.push(format!("owner {current_owner} → {owner}"));
            if !dry_run {
                let output = run_cmd("chown", &[owner, path])?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(Error::Resource(format!("chown failed: {stderr}")));
                }
            }
        }
    }

    Ok(ResourceResult {
        resource_type: "file".into(),
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
