use std::path::{Path, PathBuf};

use crate::error::Error;

/// A uniquely-named temp directory created with mode 0700 that is removed on
/// drop. Avoids predictable /tmp paths (symlink/hijack races) when the agent
/// runs as root.
pub struct ScopedTempDir {
    path: PathBuf,
}

impl ScopedTempDir {
    pub fn new(prefix: &str) -> Result<ScopedTempDir, Error> {
        use std::os::unix::fs::DirBuilderExt;

        let base = std::env::temp_dir();
        let pid = std::process::id();
        // Try a bounded number of candidates; O_EXCL via create() (no
        // following an existing path) guarantees we own the directory.
        for attempt in 0..64u32 {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            let candidate = base.join(format!("{prefix}-{pid}-{nanos}-{attempt}"));
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            match builder.create(&candidate) {
                Ok(()) => return Ok(ScopedTempDir { path: candidate }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(Error::Resource(format!(
                        "failed to create temp dir {}: {e}",
                        candidate.display()
                    )));
                }
            }
        }
        Err(Error::Resource(
            "failed to create a unique temp dir after 64 attempts".into(),
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScopedTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn creates_unique_private_dir() {
        let a = ScopedTempDir::new("verg-test").unwrap();
        let b = ScopedTempDir::new("verg-test").unwrap();
        assert_ne!(a.path(), b.path(), "each temp dir must be unique");
        assert!(a.path().is_dir());
        let mode = std::fs::metadata(a.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "temp dir must be private to owner");
    }

    #[test]
    fn removes_dir_on_drop() {
        let p;
        {
            let t = ScopedTempDir::new("verg-test").unwrap();
            p = t.path().to_path_buf();
            std::fs::write(p.join("f"), b"x").unwrap();
        }
        assert!(!p.exists(), "temp dir must be removed when dropped");
    }
}
