use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::error::Error;

use super::run_cmd;

/// Atomically write `bytes` to `path`.
///
/// Writes to a temp file in the SAME directory (so the rename is atomic on the
/// same filesystem), sets the mode (explicit `mode`, else the existing file's
/// mode, else 0644), preserves the existing file's owner/group, fsyncs the temp
/// file once (after content and metadata), then renames into place. Refuses to
/// replace a symlink. The atomic `rename` is the primary guarantee; the
/// parent-directory fsync and the `restorecon` relabel are best-effort (no-ops
/// where unsupported, e.g. non-SELinux systems and macOS).
pub fn write_atomic(path: &Path, bytes: &[u8], mode: Option<u32>) -> Result<(), Error> {
    // Refuse to silently replace a symlink with a regular file.
    if let Ok(meta) = std::fs::symlink_metadata(path)
        && meta.file_type().is_symlink()
    {
        return Err(Error::Resource(format!(
            "refusing to atomically write {}: path is a symlink",
            path.display()
        )));
    }

    let parent = path
        .parent()
        .ok_or_else(|| Error::Resource(format!("{} has no parent directory", path.display())))?;
    let existing = std::fs::metadata(path).ok();

    let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("verg");
    let tmp_path = parent.join(format!(".{file_name}.verg-tmp.{}", std::process::id()));

    // O_EXCL create: we own the temp file.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)
        .map_err(|e| {
            Error::Resource(format!(
                "failed to create temp file {}: {e}",
                tmp_path.display()
            ))
        })?;

    let staged = (|| {
        f.write_all(bytes)
            .map_err(|e| Error::Resource(format!("write temp: {e}")))?;

        let desired_mode = mode
            .or_else(|| existing.as_ref().map(|m| m.permissions().mode() & 0o7777))
            .unwrap_or(0o644);
        std::fs::set_permissions(&tmp_path, std::fs::Permissions::from_mode(desired_mode))
            .map_err(|e| Error::Resource(format!("set mode on temp: {e}")))?;

        if let Some(meta) = &existing {
            use std::os::unix::fs::MetadataExt;
            std::os::unix::fs::chown(&tmp_path, Some(meta.uid()), Some(meta.gid()))
                .map_err(|e| Error::Resource(format!("preserve owner on temp: {e}")))?;
        }

        // Single fsync after content + mode + owner so all are durable on disk
        // before the rename.
        f.sync_all()
            .map_err(|e| Error::Resource(format!("fsync temp: {e}")))?;
        Ok(())
    })();

    if let Err(e) = staged {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(e);
    }

    if let Err(e) = std::fs::rename(&tmp_path, path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(Error::Resource(format!(
            "failed to rename {} -> {}: {e}",
            tmp_path.display(),
            path.display()
        )));
    }

    // Best-effort: fsync the parent directory (the atomic rename is the primary
    // guarantee; some filesystems do not support directory fsync).
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }

    // Best-effort SELinux relabel to the policy-correct context for this path.
    let label_target = path.to_string_lossy();
    let _ = run_cmd("restorecon", &[label_target.as_ref()]);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn writes_new_file_with_default_mode() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("conf");
        write_atomic(&p, b"hello", None).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello");
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644);
    }

    #[test]
    fn writes_with_explicit_mode() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("secret");
        write_atomic(&p, b"k", Some(0o600)).unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn preserves_existing_mode_when_none_given() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("conf");
        std::fs::write(&p, b"old").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
        write_atomic(&p, b"new", None).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "existing mode should be preserved");
    }

    #[test]
    fn refuses_to_replace_a_symlink() {
        let dir = tempfile::TempDir::new().unwrap();
        let real = dir.path().join("real");
        std::fs::write(&real, b"x").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = write_atomic(&link, b"new", None).unwrap_err();
        assert!(err.to_string().contains("symlink"), "got: {err}");
        // The link target is untouched.
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "x");
    }
}
