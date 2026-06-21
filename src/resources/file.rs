use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, parse_octal_mode, run_checked, run_cmd};

pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let path = resource.prop_str_required("path")?;

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

    // Parse the desired mode up front so a freshly-created file is written with
    // the correct permissions atomically (no brief 0644 window before a separate
    // chmod, which would expose a sensitive 0600 file).
    let desired_mode = match resource.props.get("mode").and_then(|v| v.as_str()) {
        Some(mode_str) => Some(parse_octal_mode(mode_str)?),
        None => None,
    };

    // Detect mode drift against the CURRENT on-disk file BEFORE any write. The
    // content write below applies the mode atomically via write_atomic, so
    // capturing the pre-write mode here keeps the reported diff accurate (and
    // identical between dry-run and apply). Mode drift only applies to a file
    // that already exists; a new file is created with the desired mode directly.
    let mode_drift: Option<(u32, u32)> = match desired_mode {
        Some(dm) if target.exists() => {
            let current_mode = std::fs::metadata(target)
                .map_err(|e| Error::Resource(format!("failed to stat {path}: {e}")))?
                .permissions()
                .mode()
                & 0o7777;
            (current_mode != dm).then_some((current_mode, dm))
        }
        _ => None,
    };

    let mut content_written = false;
    if let Some(desired) = &desired_content {
        let current = crate::resources::read_current(target)?;
        if current.as_deref() != Some(desired.as_str()) {
            changes.push("content".to_string());
            if !dry_run {
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| Error::Resource(format!("failed to create dir: {e}")))?;
                }
                crate::resources::atomic::write_atomic(target, desired.as_bytes(), desired_mode)
                    .map_err(|e| Error::Resource(format!("failed to write {path}: {e}")))?;
                content_written = true;
            }
        }
    }

    // Report the mode change (computed pre-write). When a content write ran it
    // already set the desired mode via write_atomic, so only chmod separately
    // when no content write happened (mode-only drift on an unchanged file).
    if let Some((current_mode, desired_mode)) = mode_drift {
        changes.push(format!("mode {current_mode:04o} → {desired_mode:04o}"));
        if !dry_run && !content_written {
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(desired_mode))
                .map_err(|e| Error::Resource(format!("failed to chmod {path}: {e}")))?;
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
                run_checked("chown", &[owner, path], "chown")?;
            }
        }
    }

    Ok(ResourceResult::from_changes(
        "file",
        resource.name.clone(),
        &changes,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::ResourceStatus;
    use std::collections::HashMap;

    fn resource(name: &str, props: HashMap<String, toml::Value>) -> ResolvedResource {
        crate::resources::test_resource("file", name, props)
    }

    #[test]
    fn missing_path_is_an_error() {
        let err = execute(&resource("f", HashMap::new()), true).unwrap_err();
        assert!(err.to_string().contains("requires 'path'"), "got: {err}");
    }

    #[test]
    fn writes_content_and_is_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("conf");
        let mut props = HashMap::new();
        props.insert(
            "path".into(),
            toml::Value::String(path.to_string_lossy().into_owned()),
        );
        props.insert("content".into(), toml::Value::String("hello\n".into()));
        let r = resource("conf", props);

        // First apply writes the content (via write_atomic) and reports Changed.
        let first = execute(&r, false).unwrap();
        assert_eq!(first.status, ResourceStatus::Changed);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello\n");
        // No temp file is left behind in the directory.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("verg-tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file left behind");

        // Second apply is a no-op (Ok), proving idempotency.
        let second = execute(&r, false).unwrap();
        assert_eq!(second.status, ResourceStatus::Ok);
    }

    #[test]
    fn new_file_is_created_with_desired_mode() {
        // A new file declared with mode 0600 must be created with 0600 directly
        // (write_atomic receives the mode), never existing as 0644 first.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("secret.conf");
        let mut props = HashMap::new();
        props.insert(
            "path".into(),
            toml::Value::String(path.to_string_lossy().into_owned()),
        );
        props.insert("content".into(), toml::Value::String("token\n".into()));
        props.insert("mode".into(), toml::Value::String("0600".into()));
        let r = resource("secret", props);

        let result = execute(&r, false).unwrap();
        assert_eq!(result.status, ResourceStatus::Changed);
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o600, "new file must be created with mode 0600");

        // Idempotent: a second apply reports no change (mode already correct).
        let second = execute(&r, false).unwrap();
        assert_eq!(second.status, ResourceStatus::Ok);
    }

    #[test]
    fn existing_file_content_and_mode_drift_both_reported_on_apply() {
        // An existing file that drifts in BOTH content and mode must report both
        // in the apply diff (the mode is applied atomically with the content
        // write, but the change is still recorded), matching dry-run.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("conf");
        std::fs::write(&path, "old\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let mut props = HashMap::new();
        props.insert(
            "path".into(),
            toml::Value::String(path.to_string_lossy().into_owned()),
        );
        props.insert("content".into(), toml::Value::String("new\n".into()));
        props.insert("mode".into(), toml::Value::String("0600".into()));
        let r = resource("conf", props);

        // Dry-run reports both content and mode.
        let dry = execute(&r, true).unwrap();
        let dry_diff = dry.diff.clone().unwrap_or_default();
        assert!(dry_diff.contains("content"), "dry-run diff: {dry_diff}");
        assert!(dry_diff.contains("mode"), "dry-run diff: {dry_diff}");

        // Apply reports the SAME set (content + mode), and converges.
        let applied = execute(&r, false).unwrap();
        assert_eq!(applied.status, ResourceStatus::Changed);
        let diff = applied.diff.unwrap_or_default();
        assert!(diff.contains("content"), "apply diff: {diff}");
        assert!(
            diff.contains("mode"),
            "apply diff must report mode too: {diff}"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new\n");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o600);

        // Idempotent.
        assert_eq!(execute(&r, false).unwrap().status, ResourceStatus::Ok);
    }
}
