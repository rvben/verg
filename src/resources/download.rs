use std::path::Path;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, ResourceStatus, run_cmd};

/// Downloads a file from a URL and places it at the destination.
///
/// Properties:
///   url      - URL to download from
///   dest     - Destination path on target
///   mode     - File permissions (octal, e.g. "0755")
///   owner    - File owner
///   extract  - If true, extract archive and place contents at dest (default: false)
///   checksum - Optional sha256 checksum to verify download
///   state    - "present" or "absent" (default: "present")
pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let url = resource
        .props
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Resource("download resource requires 'url'".into()))?;

    let dest = resource
        .props
        .get("dest")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Resource("download resource requires 'dest'".into()))?;

    let state = resource
        .props
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("present");

    if state == "absent" {
        return remove(dest, &resource.name, dry_run);
    }

    let extract = resource
        .props
        .get("extract")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let checksum = resource.props.get("checksum").and_then(|v| v.as_str());
    let mode_str = resource.props.get("mode").and_then(|v| v.as_str());
    let owner = resource.props.get("owner").and_then(|v| v.as_str());

    let mut changes = Vec::new();

    if Path::new(dest).exists() {
        // Verify checksum if specified
        if let Some(expected) = checksum {
            let output = run_cmd("sha256sum", &[dest])?;
            let actual = String::from_utf8_lossy(&output.stdout);
            let actual_hash = actual.split_whitespace().next().unwrap_or("");
            if actual_hash != expected {
                changes.push("checksum mismatch (re-download)".to_string());
                if !dry_run {
                    std::fs::remove_file(dest).map_err(|e| {
                        Error::Resource(format!("failed to remove stale {dest}: {e}"))
                    })?;
                }
            }
        }

        // Verify mode if specified
        if let Some(mode_str) = mode_str {
            let desired_mode = u32::from_str_radix(mode_str, 8)
                .map_err(|_| Error::Resource(format!("invalid mode: {mode_str}")))?;
            if let Ok(meta) = std::fs::metadata(dest) {
                use std::os::unix::fs::PermissionsExt;
                let current_mode = meta.permissions().mode() & 0o7777;
                if current_mode != desired_mode {
                    changes.push(format!("mode {current_mode:04o} → {desired_mode:04o}"));
                    if !dry_run {
                        run_cmd("chmod", &[mode_str, dest])?;
                    }
                }
            }
        }

        // Verify owner if specified
        if let Some(owner) = owner {
            let ls_output = run_cmd("ls", &["-ld", dest])?;
            let ls_line = String::from_utf8_lossy(&ls_output.stdout);
            let current_owner = ls_line.split_whitespace().nth(2).unwrap_or("");
            if current_owner != owner {
                changes.push(format!("owner {current_owner} → {owner}"));
                if !dry_run {
                    run_cmd("chown", &[owner, dest])?;
                }
            }
        }

        // If only metadata drifted, no re-download needed
        if changes.is_empty() || !changes.iter().any(|c| c.contains("re-download")) {
            return Ok(ResourceResult {
                resource_type: "download".into(),
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
            });
        }
    }

    if dry_run {
        if changes.is_empty() {
            changes.push(format!("would download {url} → {dest}"));
        }
        return Ok(ResourceResult {
            resource_type: "download".into(),
            name: resource.name.clone(),
            status: ResourceStatus::Changed,
            diff: Some(changes.join(", ")),
            from: None,
            to: None,
            error: None,
        });
    }

    // Create parent directory
    if let Some(parent) = Path::new(dest).parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::Resource(format!("failed to create directory: {e}")))?;
    }

    if extract {
        download_and_extract(resource, url, dest, checksum, &mut changes)?;
    } else {
        download_direct(url, dest, checksum, &mut changes)?;
    }

    // Set mode
    if let Some(mode_str) = mode_str {
        run_cmd("chmod", &[mode_str, dest])?;
    }

    // Set owner
    if let Some(owner) = owner {
        run_cmd("chown", &[owner, dest])?;
    }

    Ok(ResourceResult {
        resource_type: "download".into(),
        name: resource.name.clone(),
        status: ResourceStatus::Changed,
        diff: Some(changes.join(", ")),
        from: None,
        to: None,
        error: None,
    })
}

fn download_and_extract(
    resource: &ResolvedResource,
    url: &str,
    dest: &str,
    checksum: Option<&str>,
    changes: &mut Vec<String>,
) -> Result<(), Error> {
    let tmp_path = format!("/tmp/verg-download-{}", resource.name);
    let extract_dir = "/tmp/verg-extract";

    let output = run_cmd("curl", &["-fSL", "-o", &tmp_path, url])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Resource(format!("download failed: {stderr}")));
    }

    verify_checksum(&tmp_path, checksum)?;

    // Detect archive type and extract
    if tmp_path.ends_with(".zip") || url.ends_with(".zip") {
        let output = run_cmd("unzip", &["-o", &tmp_path, "-d", extract_dir])?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let _ = std::fs::remove_file(&tmp_path);
            return Err(Error::Resource(format!("unzip failed: {stderr}")));
        }
    } else if url.ends_with(".tar.gz") || url.ends_with(".tgz") {
        run_cmd("mkdir", &["-p", extract_dir])?;
        let output = run_cmd("tar", &["-xzf", &tmp_path, "-C", extract_dir])?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let _ = std::fs::remove_file(&tmp_path);
            return Err(Error::Resource(format!("tar extract failed: {stderr}")));
        }
    } else {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(Error::Resource(format!(
            "unsupported archive format for {url}. Supported: .zip, .tar.gz, .tgz"
        )));
    }

    // Find the extracted binary matching the resource name or dest basename
    let dest_basename = Path::new(dest)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let extracted = find_extracted_file(extract_dir, &dest_basename)?;

    let output = run_cmd("cp", &[&extracted.to_string_lossy(), dest])?;
    if !output.status.success() {
        return Err(Error::Resource(format!(
            "failed to copy extracted file to {dest}"
        )));
    }

    // Cleanup
    let _ = std::fs::remove_file(&tmp_path);
    let _ = std::fs::remove_dir_all(extract_dir);

    changes.push(format!("downloaded and extracted → {dest}"));
    Ok(())
}

fn download_direct(
    url: &str,
    dest: &str,
    checksum: Option<&str>,
    changes: &mut Vec<String>,
) -> Result<(), Error> {
    let output = run_cmd("curl", &["-fSL", "-o", dest, url])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Resource(format!("download failed: {stderr}")));
    }

    verify_checksum(dest, checksum)?;

    changes.push(format!("downloaded → {dest}"));
    Ok(())
}

fn verify_checksum(path: &str, checksum: Option<&str>) -> Result<(), Error> {
    let Some(expected) = checksum else {
        return Ok(());
    };

    let output = run_cmd("sha256sum", &[path])?;
    let actual = String::from_utf8_lossy(&output.stdout);
    let actual_hash = actual.split_whitespace().next().unwrap_or("");
    if actual_hash != expected {
        let _ = std::fs::remove_file(path);
        return Err(Error::Resource(format!(
            "checksum mismatch: expected {expected}, got {actual_hash}"
        )));
    }
    Ok(())
}

fn find_extracted_file(
    extract_dir: &str,
    dest_basename: &str,
) -> Result<std::path::PathBuf, Error> {
    let mut found = None;

    if let Ok(entries) = std::fs::read_dir(extract_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == dest_basename || name.starts_with(dest_basename) {
                found = Some(entry.path());
                break;
            }
        }
        // If not found by name, take the first regular file
        if found.is_none()
            && let Ok(entries) = std::fs::read_dir(extract_dir)
        {
            for entry in entries.flatten() {
                if entry.path().is_file() {
                    found = Some(entry.path());
                    break;
                }
            }
        }
    }

    found.ok_or_else(|| Error::Resource("no files found after extraction".into()))
}

fn remove(dest: &str, name: &str, dry_run: bool) -> Result<ResourceResult, Error> {
    if !Path::new(dest).exists() {
        return Ok(ResourceResult {
            resource_type: "download".into(),
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
            resource_type: "download".into(),
            name: name.to_string(),
            status: ResourceStatus::Changed,
            diff: Some(format!("would remove {dest}")),
            from: None,
            to: None,
            error: None,
        });
    }

    std::fs::remove_file(dest)
        .map_err(|e| Error::Resource(format!("failed to remove {dest}: {e}")))?;

    Ok(ResourceResult {
        resource_type: "download".into(),
        name: name.to_string(),
        status: ResourceStatus::Changed,
        diff: Some(format!("removed {dest}")),
        from: None,
        to: None,
        error: None,
    })
}
