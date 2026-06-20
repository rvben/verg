use std::path::Path;
use std::sync::OnceLock;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, run_checked, run_cmd};

/// Cache for the distribution suite detected via `lsb_release -cs`. The
/// distro suite is invariant for the lifetime of the agent process, so the
/// successful probe runs at most once. Only success is cached; a failure
/// returns the exact live error every time, so both error paths
/// (`run_cmd` failure and a non-zero exit) keep their original message.
static SUITE_CACHE: OnceLock<String> = OnceLock::new();

fn detect_suite() -> Result<String, Error> {
    if let Some(suite) = SUITE_CACHE.get() {
        return Ok(suite.clone());
    }
    let output = run_cmd("lsb_release", &["-cs"])?;
    if !output.status.success() {
        return Err(Error::Resource(
            "failed to detect distribution suite via lsb_release -cs".to_string(),
        ));
    }
    let suite = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let _ = SUITE_CACHE.set(suite.clone());
    Ok(suite)
}

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
///
/// GPG key rotation: the key file is only re-fetched when absent. To rotate a
/// signing key, apply the resource with `state = "absent"` (removes the keyring
/// and list) and then `state = "present"` to fetch the new key.
pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let name = resource.prop_str_required("name")?;

    let state = resource.prop_str_or("state", "present");

    let keyring_path = format!("/etc/apt/keyrings/{name}.asc");
    let list_path = format!("/etc/apt/sources.list.d/{name}.list");

    if state == "absent" {
        return remove(name, &keyring_path, &list_path, dry_run);
    }

    let url = resource.prop_str_required("url")?;

    let gpg_key = resource.prop_str_required("gpg_key")?;

    let component = resource.prop_str_or("component", "stable");

    let arch = resource.prop_str_or("arch", "amd64");

    // Detect suite from the system if not specified
    let suite = if let Some(s) = resource.props.get("suite").and_then(|v| v.as_str()) {
        s.to_string()
    } else {
        detect_suite()?
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
            let body = format!("{repo_line}\n");
            crate::resources::atomic::write_atomic(
                std::path::Path::new(&list_path),
                body.as_bytes(),
                None,
            )
            .map_err(|e| Error::Resource(format!("failed to write {list_path}: {e}")))?;
            // Update package lists
            run_checked("apt-get", &["update", "-qq"], "apt-get update")?;
        }
    }

    Ok(ResourceResult::from_changes(
        "apt_repo",
        resource.name.clone(),
        &changes,
    ))
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

    Ok(ResourceResult::from_changes(
        "apt_repo",
        name.to_string(),
        &changes,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn resource(props: HashMap<String, toml::Value>) -> ResolvedResource {
        crate::resources::test_resource("apt_repo", "t", props)
    }

    #[test]
    fn missing_name_is_an_error() {
        let err = execute(&resource(HashMap::new()), true).unwrap_err();
        assert!(err.to_string().contains("requires 'name'"), "got: {err}");
    }
}
