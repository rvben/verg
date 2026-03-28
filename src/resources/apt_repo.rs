use std::path::Path;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, ResourceStatus, run_cmd};

/// Manages an APT repository with GPG key.
///
/// Properties:
///   name     - Repository identifier (used for filenames)
///   url      - Base URL of the repository (e.g. "https://download.docker.com/linux/ubuntu")
///   gpg_key  - URL to the GPG signing key
///   suite    - Distribution suite (default: auto-detected via lsb_release -cs)
///   component - Repository component (default: "stable")
///   arch     - Architecture (default: "amd64")
///   state    - "present" or "absent" (default: "present")
pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let name = resource
        .props
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Resource("apt_repo resource requires 'name'".into()))?;

    let state = resource
        .props
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("present");

    let keyring_path = format!("/etc/apt/keyrings/{name}.asc");
    let list_path = format!("/etc/apt/sources.list.d/{name}.list");

    if state == "absent" {
        return remove(name, &keyring_path, &list_path, dry_run);
    }

    let url = resource
        .props
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Resource("apt_repo resource requires 'url'".into()))?;

    let gpg_key = resource
        .props
        .get("gpg_key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Resource("apt_repo resource requires 'gpg_key'".into()))?;

    let component = resource
        .props
        .get("component")
        .and_then(|v| v.as_str())
        .unwrap_or("stable");

    let arch = resource
        .props
        .get("arch")
        .and_then(|v| v.as_str())
        .unwrap_or("amd64");

    // Detect suite from the system if not specified
    let suite = if let Some(s) = resource.props.get("suite").and_then(|v| v.as_str()) {
        s.to_string()
    } else {
        let output = run_cmd("lsb_release", &["-cs"])?;
        if !output.status.success() {
            return Err(Error::Resource(
                "failed to detect distribution suite via lsb_release -cs".into(),
            ));
        }
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };

    let mut changes = Vec::new();

    // Step 1: GPG key
    if !Path::new(&keyring_path).exists() {
        changes.push(format!("gpg key → {keyring_path}"));
        if !dry_run {
            run_cmd("mkdir", &["-p", "/etc/apt/keyrings"])?;
            let output = run_cmd("curl", &["-fsSL", "-o", &keyring_path, gpg_key])?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(Error::Resource(format!(
                    "failed to download GPG key from {gpg_key}: {stderr}"
                )));
            }
            run_cmd("chmod", &["a+r", &keyring_path])?;
        }
    }

    // Step 2: Repository list file
    let repo_line = format!("deb [arch={arch} signed-by={keyring_path}] {url} {suite} {component}");
    let needs_update = if Path::new(&list_path).exists() {
        let current = std::fs::read_to_string(&list_path).unwrap_or_default();
        current.trim() != repo_line
    } else {
        true
    };

    if needs_update {
        changes.push(format!("repo → {list_path}"));
        if !dry_run {
            std::fs::write(&list_path, format!("{repo_line}\n"))
                .map_err(|e| Error::Resource(format!("failed to write {list_path}: {e}")))?;
            // Update package lists
            let output = run_cmd("apt-get", &["update", "-qq"])?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr);
                return Err(Error::Resource(format!("apt-get update failed: {stderr}")));
            }
        }
    }

    Ok(ResourceResult {
        resource_type: "apt_repo".into(),
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

fn remove(
    name: &str,
    keyring_path: &str,
    list_path: &str,
    dry_run: bool,
) -> Result<ResourceResult, Error> {
    let mut changes = Vec::new();

    if Path::new(keyring_path).exists() {
        changes.push(format!("-{keyring_path}"));
        if !dry_run {
            std::fs::remove_file(keyring_path)
                .map_err(|e| Error::Resource(format!("failed to remove {keyring_path}: {e}")))?;
        }
    }

    if Path::new(list_path).exists() {
        changes.push(format!("-{list_path}"));
        if !dry_run {
            std::fs::remove_file(list_path)
                .map_err(|e| Error::Resource(format!("failed to remove {list_path}: {e}")))?;
        }
    }

    Ok(ResourceResult {
        resource_type: "apt_repo".into(),
        name: name.to_string(),
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
