use std::path::Path;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, atomic, run_checked, run_cmd};

/// Return the fstab line whose mountpoint (whitespace-split field index 1)
/// exactly equals `path`. Blank lines and full-line comments (first
/// non-whitespace char is `#`) are skipped. Returns `None` when no such
/// line is found.
fn fstab_has_entry<'a>(content: &'a str, path: &str) -> Option<&'a str> {
    content.lines().find(|line| {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            return false;
        }
        let mut fields = trimmed.split_whitespace();
        // Field index 0 is device, index 1 is mountpoint.
        fields.next();
        fields.next().is_some_and(|mp| mp == path)
    })
}

/// Return a new fstab content string with the line for `path` replaced by
/// `desired_line`, or with `desired_line` appended when no such line exists.
/// Blank lines, comments, and all other entries are preserved in their
/// original order. A trailing newline is always ensured.
fn upsert_fstab(content: &str, desired_line: &str, path: &str) -> String {
    let mut found = false;
    let mut lines: Vec<String> = content
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return line.to_string();
            }
            let mut fields = trimmed.split_whitespace();
            fields.next();
            if fields.next().is_some_and(|mp| mp == path) {
                found = true;
                desired_line.to_string()
            } else {
                line.to_string()
            }
        })
        .collect();

    if !found {
        lines.push(desired_line.to_string());
    }

    let mut result = lines.join("\n");
    if !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

/// Return a new fstab content string with the line for `path` removed.
/// All other lines (including blank lines and comments) are preserved.
fn remove_fstab_entry(content: &str, path: &str) -> String {
    let lines: Vec<&str> = content
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return true;
            }
            let mut fields = trimmed.split_whitespace();
            fields.next();
            fields.next().is_none_or(|mp| mp != path)
        })
        .collect();

    let mut result = lines.join("\n");
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let device = resource.prop_str_required("device")?;
    let path = resource.prop_str_required("path")?;
    let fstype = resource.prop_str_required("fstype")?;
    let options = resource.prop_str_or("options", "defaults");
    let dump = resource.prop_str_or("dump", "0");
    let pass = resource.prop_str_or("pass", "0");
    let state = resource.prop_str_or("state", "mounted");
    let name = resource.name.clone();

    if path.chars().any(|c| c.is_ascii_whitespace()) {
        return Ok(ResourceResult::failed(
            "mount",
            name,
            "mountpoint with whitespace is not supported",
        ));
    }

    // Only a missing /etc/fstab is treated as empty. Any other read error
    // (permission, I/O) must abort: treating it as empty would make upsert
    // produce a single-line file and atomically clobber the real fstab.
    let fstab_content = match std::fs::read_to_string("/etc/fstab") {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(Error::Resource(format!("failed to read /etc/fstab: {e}")));
        }
    };

    let desired_line = format!("{device}\t{path}\t{fstype}\t{options}\t{dump}\t{pass}");

    let mut changes = Vec::new();

    match state {
        "mounted" => {
            // Phase 1: ensure fstab entry is correct.
            let existing_line = fstab_has_entry(&fstab_content, path);
            if existing_line != Some(desired_line.as_str()) {
                changes.push(format!("fstab entry for {path}"));
                if !dry_run {
                    let new_content = upsert_fstab(&fstab_content, &desired_line, path);
                    atomic::write_atomic(Path::new("/etc/fstab"), new_content.as_bytes(), None)
                        .map_err(|e| Error::Resource(format!("failed to write /etc/fstab: {e}")))?;
                }
            }

            // Phase 2: ensure the mountpoint is mounted.
            let mounted = run_cmd("mountpoint", &["-q", path])?.status.success();
            if !mounted {
                changes.push(format!("mount {path}"));
                if !dry_run {
                    run_checked("mount", &[path], "mount")?;
                }
            }
        }
        "absent" => {
            // Phase 1: unmount if currently mounted.
            let mounted = run_cmd("mountpoint", &["-q", path])?.status.success();
            if mounted {
                changes.push(format!("unmount {path}"));
                if !dry_run {
                    run_checked("umount", &[path], "umount")?;
                }
            }

            // Phase 2: remove fstab entry if present.
            if fstab_has_entry(&fstab_content, path).is_some() {
                changes.push(format!("remove fstab entry for {path}"));
                if !dry_run {
                    let new_content = remove_fstab_entry(&fstab_content, path);
                    atomic::write_atomic(Path::new("/etc/fstab"), new_content.as_bytes(), None)
                        .map_err(|e| Error::Resource(format!("failed to write /etc/fstab: {e}")))?;
                }
            }
        }
        other => {
            return Err(Error::Resource(format!(
                "mount resource: unsupported state '{other}'; expected 'mounted' or 'absent'"
            )));
        }
    }

    Ok(ResourceResult::from_changes("mount", name, &changes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_resource(props: HashMap<String, toml::Value>) -> ResolvedResource {
        crate::resources::test_resource("mount", "test", props)
    }

    // --- fstab_has_entry ---

    #[test]
    fn fstab_has_entry_finds_by_exact_mountpoint() {
        let content =
            "/dev/sda1\t/boot\text4\tdefaults\t0\t2\n/dev/sda2\t/\text4\tdefaults\t0\t1\n";
        assert_eq!(
            fstab_has_entry(content, "/boot"),
            Some("/dev/sda1\t/boot\text4\tdefaults\t0\t2")
        );
        assert_eq!(
            fstab_has_entry(content, "/"),
            Some("/dev/sda2\t/\text4\tdefaults\t0\t1")
        );
    }

    #[test]
    fn fstab_has_entry_returns_none_when_absent() {
        let content = "/dev/sda1\t/boot\text4\tdefaults\t0\t2\n";
        assert!(fstab_has_entry(content, "/data").is_none());
    }

    #[test]
    fn fstab_has_entry_skips_comment_lines() {
        let content =
            "# /dev/sdb1\t/data\text4\tdefaults\t0\t0\n/dev/sda1\t/boot\text4\tdefaults\t0\t2\n";
        assert!(fstab_has_entry(content, "/data").is_none());
        assert!(fstab_has_entry(content, "/boot").is_some());
    }

    #[test]
    fn fstab_has_entry_skips_blank_lines() {
        let content = "\n\n/dev/sda1\t/boot\text4\tdefaults\t0\t2\n\n";
        assert!(fstab_has_entry(content, "/boot").is_some());
    }

    #[test]
    fn fstab_has_entry_no_prefix_match_for_slash_data() {
        // /data must NOT match /data2 and vice versa.
        let content = "/dev/sdb1\t/data2\text4\tdefaults\t0\t0\n";
        assert!(
            fstab_has_entry(content, "/data").is_none(),
            "/data must not match /data2 entry"
        );
        let content2 = "/dev/sdb1\t/data\text4\tdefaults\t0\t0\n";
        assert!(
            fstab_has_entry(content2, "/data2").is_none(),
            "/data2 must not match /data entry"
        );
    }

    // --- upsert_fstab ---

    #[test]
    fn upsert_fstab_appends_when_absent() {
        let content = "/dev/sda1\t/boot\text4\tdefaults\t0\t2\n";
        let desired = "/dev/sdb1\t/data\text4\tdefaults\t0\t0";
        let result = upsert_fstab(content, desired, "/data");
        assert!(
            result.contains("/dev/sda1\t/boot"),
            "original line preserved"
        );
        assert!(
            result.contains("/dev/sdb1\t/data\text4\tdefaults\t0\t0"),
            "desired line appended"
        );
        assert!(result.ends_with('\n'), "trailing newline ensured");
    }

    #[test]
    fn upsert_fstab_replaces_existing_line() {
        let content =
            "/dev/sdb1\t/data\text4\tdefaults\t0\t0\n/dev/sda1\t/boot\text4\tdefaults\t0\t2\n";
        let desired = "/dev/sdb2\t/data\txfs\tnoatime\t0\t0";
        let result = upsert_fstab(content, desired, "/data");
        // Only the new line for /data is present (not the old one).
        assert!(
            result.contains("/dev/sdb2\t/data\txfs\tnoatime"),
            "desired line must be in result"
        );
        assert!(
            !result.contains("/dev/sdb1\t/data"),
            "old line must be removed"
        );
        // Other lines preserved.
        assert!(result.contains("/dev/sda1\t/boot"), "/boot line preserved");
        assert!(result.ends_with('\n'), "trailing newline ensured");
    }

    #[test]
    fn upsert_fstab_preserves_comments_and_blank_lines() {
        let content = "# /etc/fstab\n\n/dev/sda1\t/boot\text4\tdefaults\t0\t2\n";
        let desired = "/dev/sdb1\t/data\text4\tdefaults\t0\t0";
        let result = upsert_fstab(content, desired, "/data");
        assert!(result.contains("# /etc/fstab"), "comment preserved");
        assert!(result.contains("/dev/sda1\t/boot"), "other entry preserved");
        assert!(result.contains(desired), "new entry appended");
    }

    // --- remove_fstab_entry ---

    #[test]
    fn remove_fstab_entry_removes_matching_line() {
        let content =
            "/dev/sda1\t/boot\text4\tdefaults\t0\t2\n/dev/sdb1\t/data\text4\tdefaults\t0\t0\n";
        let result = remove_fstab_entry(content, "/data");
        assert!(!result.contains("/data"), "/data line removed");
        assert!(result.contains("/dev/sda1\t/boot"), "/boot line preserved");
    }

    #[test]
    fn remove_fstab_entry_preserves_comments_and_blank_lines() {
        let content = "# /etc/fstab\n\n/dev/sda1\t/boot\text4\tdefaults\t0\t2\n/dev/sdb1\t/data\text4\tdefaults\t0\t0\n";
        let result = remove_fstab_entry(content, "/data");
        assert!(result.contains("# /etc/fstab"), "comment preserved");
        assert!(result.contains("/dev/sda1\t/boot"), "/boot preserved");
        assert!(!result.contains("/data"), "/data removed");
    }

    #[test]
    fn remove_fstab_entry_noop_when_not_present() {
        let content = "/dev/sda1\t/boot\text4\tdefaults\t0\t2\n";
        let result = remove_fstab_entry(content, "/data");
        assert!(result.contains("/dev/sda1\t/boot"), "content unchanged");
    }

    #[test]
    fn remove_fstab_entry_does_not_remove_prefix_sibling() {
        let content =
            "/dev/sdb1\t/data2\text4\tdefaults\t0\t0\n/dev/sda1\t/boot\text4\tdefaults\t0\t2\n";
        let result = remove_fstab_entry(content, "/data");
        assert!(result.contains("/data2"), "/data2 must NOT be removed");
    }

    // --- executor validation ---

    #[test]
    fn whitespace_in_mountpoint_returns_failed() {
        let mut props = HashMap::new();
        props.insert("device".into(), toml::Value::String("/dev/sdb1".into()));
        props.insert("path".into(), toml::Value::String("/mnt/my mount".into()));
        props.insert("fstype".into(), toml::Value::String("ext4".into()));
        let resource = make_resource(props);
        let result = execute(&resource, true).unwrap();
        assert_eq!(result.status, crate::resources::ResourceStatus::Failed);
        assert!(
            result.error.as_deref().unwrap_or("").contains("whitespace"),
            "error must mention whitespace, got: {:?}",
            result.error
        );
    }

    #[test]
    fn missing_device_returns_error() {
        let mut props = HashMap::new();
        props.insert("path".into(), toml::Value::String("/data".into()));
        props.insert("fstype".into(), toml::Value::String("ext4".into()));
        let resource = make_resource(props);
        let result = execute(&resource, true);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("device"));
    }

    #[test]
    fn missing_path_returns_error() {
        let mut props = HashMap::new();
        props.insert("device".into(), toml::Value::String("/dev/sdb1".into()));
        props.insert("fstype".into(), toml::Value::String("ext4".into()));
        let resource = make_resource(props);
        let result = execute(&resource, true);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("path"));
    }

    #[test]
    fn missing_fstype_returns_error() {
        let mut props = HashMap::new();
        props.insert("device".into(), toml::Value::String("/dev/sdb1".into()));
        props.insert("path".into(), toml::Value::String("/data".into()));
        let resource = make_resource(props);
        let result = execute(&resource, true);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("fstype"));
    }
}
