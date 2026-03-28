use std::path::Path;

use crate::error::Error;

/// Initialize a verg project at the given path.
/// The path IS the verg project directory (not a parent).
pub fn run(path: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(path.join("state"))
        .map_err(|e| Error::Config(format!("failed to create state dir: {e}")))?;
    std::fs::create_dir_all(path.join("groups"))
        .map_err(|e| Error::Config(format!("failed to create groups dir: {e}")))?;
    std::fs::create_dir_all(path.join("files"))
        .map_err(|e| Error::Config(format!("failed to create files dir: {e}")))?;
    std::fs::create_dir_all(path.join("templates"))
        .map_err(|e| Error::Config(format!("failed to create templates dir: {e}")))?;

    let hosts = "# Static host inventory\n# [hosts.example]\n# address = \"192.168.1.10\"\n# user = \"root\"\n# groups = [\"web\", \"prod\"]\n";
    std::fs::write(path.join("hosts.toml"), hosts)
        .map_err(|e| Error::Config(format!("failed to write hosts.toml: {e}")))?;

    let base_state = "# Base state applied to all hosts\n# [resource.pkg.essential]\n# names = [\"curl\", \"htop\"]\n# state = \"present\"\n";
    std::fs::write(path.join("state").join("base.toml"), base_state)
        .map_err(|e| Error::Config(format!("failed to write base.toml: {e}")))?;

    eprintln!("Initialized verg project at {}", path.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn init_creates_directory_structure() {
        let dir = TempDir::new().unwrap();
        let verg_dir = dir.path().join("verg");
        run(&verg_dir).unwrap();

        assert!(verg_dir.join("hosts.toml").exists());
        assert!(verg_dir.join("state/base.toml").exists());
        assert!(verg_dir.join("groups").is_dir());
        assert!(verg_dir.join("files").is_dir());
        assert!(verg_dir.join("templates").is_dir());
    }
}
