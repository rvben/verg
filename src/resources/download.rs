use std::path::Path;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, run_checked, run_cmd};

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

    if extract && checksum.is_none() {
        return Err(Error::Resource(
            "download with extract = true requires a `checksum` (extracting an archive as root \
             requires pinning its exact bytes)"
                .into(),
        ));
    }

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
                    changes.push(format!("mode {current_mode:04o} -> {desired_mode:04o}"));
                    if !dry_run {
                        run_checked("chmod", &[mode_str, dest], "chmod")?;
                    }
                }
            }
        }

        // Verify owner if specified
        if let Some(owner) = owner
            && Path::new(dest).exists()
        {
            let ls_output = run_cmd("ls", &["-ld", dest])?;
            let ls_line = String::from_utf8_lossy(&ls_output.stdout);
            let current_owner = ls_line.split_whitespace().nth(2).unwrap_or("");
            if current_owner != owner {
                changes.push(format!("owner {current_owner} -> {owner}"));
                if !dry_run {
                    run_checked("chown", &[owner, dest], "chown")?;
                }
            }
        }

        // If only metadata drifted, no re-download needed
        if changes.is_empty() || !changes.iter().any(|c| c.contains("re-download")) {
            return Ok(ResourceResult::from_changes(
                "download",
                resource.name.clone(),
                &changes,
            ));
        }
    }

    if dry_run {
        if changes.is_empty() {
            changes.push(format!("would download {url} -> {dest}"));
        }
        return Ok(ResourceResult::changed(
            "download",
            resource.name.clone(),
            changes.join(", "),
        ));
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
        run_checked("chmod", &[mode_str, dest], "chmod")?;
    }

    // Set owner
    if let Some(owner) = owner {
        run_checked("chown", &[owner, dest], "chown")?;
    }

    Ok(ResourceResult::changed(
        "download",
        resource.name.clone(),
        changes.join(", "),
    ))
}

fn download_and_extract(
    _resource: &ResolvedResource,
    url: &str,
    dest: &str,
    checksum: Option<&str>,
    changes: &mut Vec<String>,
) -> Result<(), Error> {
    use super::tempdir::ScopedTempDir;

    let dl_dir = ScopedTempDir::new("verg-download")?;
    let extract = ScopedTempDir::new("verg-extract")?;
    let tmp_path = dl_dir.path().join("archive");
    let tmp_path = tmp_path.to_string_lossy().to_string();
    let extract_dir = extract.path().to_string_lossy().to_string();

    run_checked(
        "curl",
        &[
            "-fSL",
            "-m",
            "300",
            "--max-filesize",
            "536870912",
            "-o",
            &tmp_path,
            url,
        ],
        "download",
    )?;

    // checksum is guaranteed Some for the extract path (enforced in execute).
    verify_checksum(&tmp_path, checksum)?;

    if url.ends_with(".zip") {
        // -o overwrite, -d into our private dir. unzip does not restore owner.
        run_checked("unzip", &["-o", &tmp_path, "-d", &extract_dir], "unzip")?;
    } else if url.ends_with(".tar.gz") || url.ends_with(".tgz") {
        run_checked(
            "tar",
            &[
                "--no-same-owner",
                "--no-same-permissions",
                "--no-overwrite-dir",
                "-xzf",
                &tmp_path,
                "-C",
                &extract_dir,
            ],
            "tar extract",
        )?;
    } else {
        return Err(Error::Resource(format!(
            "unsupported archive format for {url}. Supported: .zip, .tar.gz, .tgz"
        )));
    }

    let dest_basename = Path::new(dest)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let extracted = find_extracted_file(&extract_dir, &dest_basename)?;

    // Reject a match that resolves outside our extraction dir (symlink escape).
    let canon_extract = std::fs::canonicalize(&extract_dir)
        .map_err(|e| Error::Resource(format!("failed to resolve extract dir: {e}")))?;
    let canon_file = std::fs::canonicalize(&extracted)
        .map_err(|e| Error::Resource(format!("failed to resolve extracted file: {e}")))?;
    if !canon_file.starts_with(&canon_extract) {
        return Err(Error::Resource(format!(
            "refusing to install '{}': extracted path escapes the extraction directory",
            extracted.display()
        )));
    }

    let output = run_cmd("cp", &[&canon_file.to_string_lossy(), dest])?;
    if !output.status.success() {
        return Err(Error::Resource(format!(
            "failed to copy extracted file to {dest}"
        )));
    }

    changes.push(format!("downloaded and extracted -> {dest}"));
    Ok(())
}

fn download_direct(
    url: &str,
    dest: &str,
    checksum: Option<&str>,
    changes: &mut Vec<String>,
) -> Result<(), Error> {
    run_checked("curl", &["-fSL", "-o", dest, url], "download")?;

    verify_checksum(dest, checksum)?;

    changes.push(format!("downloaded -> {dest}"));
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
    let mut names = Vec::new();
    let entries = std::fs::read_dir(extract_dir)
        .map_err(|e| Error::Resource(format!("failed to read {extract_dir}: {e}")))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // Exact match wins immediately.
        if name == dest_basename {
            return Ok(entry.path());
        }
        names.push(name);
    }
    Err(Error::Resource(format!(
        "no extracted file named '{dest_basename}' (archive contained: {}). \
         Set `dest` to one of these names so verg knows which file to install.",
        names.join(", ")
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracted_file_requires_name_match() {
        let dir = tempfile::TempDir::new().unwrap();
        // Two unrelated files, neither matching the requested basename.
        std::fs::write(dir.path().join("LICENSE"), b"x").unwrap();
        std::fs::write(dir.path().join("README.md"), b"y").unwrap();
        let err = find_extracted_file(&dir.path().to_string_lossy(), "mytool").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("LICENSE") || msg.contains("README"),
            "got: {msg}"
        );
        assert!(
            msg.contains("mytool"),
            "error should name the requested file: {msg}"
        );
    }

    #[test]
    fn extracted_file_matches_by_basename() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("mytool"), b"bin").unwrap();
        std::fs::write(dir.path().join("LICENSE"), b"x").unwrap();
        let found = find_extracted_file(&dir.path().to_string_lossy(), "mytool").unwrap();
        assert_eq!(found.file_name().unwrap(), "mytool");
    }

    #[test]
    fn extract_without_checksum_is_rejected() {
        // Extracting an archive as root requires a pinned checksum.
        let mut props = std::collections::HashMap::new();
        props.insert(
            "url".to_string(),
            toml::Value::String("https://example.test/a.tar.gz".into()),
        );
        props.insert(
            "dest".to_string(),
            toml::Value::String("/opt/mytool".into()),
        );
        props.insert("extract".to_string(), toml::Value::Boolean(true));
        let r = ResolvedResource {
            resource_type: "download".into(),
            name: "mytool".into(),
            props,
            after: vec![],
            notify: vec![],
            when: None,
            handler: false,
            register: None,
            sensitive: false,
        };
        let err = execute(&r, true).unwrap_err();
        assert!(err.to_string().contains("checksum"), "got: {err}");
    }
}

fn remove(dest: &str, name: &str, dry_run: bool) -> Result<ResourceResult, Error> {
    if !Path::new(dest).exists() {
        return Ok(ResourceResult::ok("download", name.to_string()));
    }

    if dry_run {
        return Ok(ResourceResult::changed(
            "download",
            name.to_string(),
            format!("would remove {dest}"),
        ));
    }

    std::fs::remove_file(dest)
        .map_err(|e| Error::Resource(format!("failed to remove {dest}: {e}")))?;

    Ok(ResourceResult::changed(
        "download",
        name.to_string(),
        format!("removed {dest}"),
    ))
}
