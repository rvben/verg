use std::path::Path;

use crate::error::Error;

pub fn run(path: &Path) -> Result<(), Error> {
    let verg_dir = path.join("verg");

    std::fs::create_dir_all(verg_dir.join("state"))
        .map_err(|e| Error::Config(format!("failed to create state dir: {e}")))?;
    std::fs::create_dir_all(verg_dir.join("groups"))
        .map_err(|e| Error::Config(format!("failed to create groups dir: {e}")))?;
    std::fs::create_dir_all(verg_dir.join("files"))
        .map_err(|e| Error::Config(format!("failed to create files dir: {e}")))?;
    std::fs::create_dir_all(verg_dir.join("templates"))
        .map_err(|e| Error::Config(format!("failed to create templates dir: {e}")))?;

    let hosts = "# Static host inventory\n# [hosts.example]\n# address = \"192.168.1.10\"\n# user = \"root\"\n# groups = [\"web\", \"prod\"]\n";
    std::fs::write(verg_dir.join("hosts.toml"), hosts)
        .map_err(|e| Error::Config(format!("failed to write hosts.toml: {e}")))?;

    let base_state = "# Base state applied to all hosts\n# [resource.pkg.essential]\n# names = [\"curl\", \"htop\"]\n# state = \"present\"\n";
    std::fs::write(verg_dir.join("state").join("base.toml"), base_state)
        .map_err(|e| Error::Config(format!("failed to write base.toml: {e}")))?;

    eprintln!("Initialized verg project at {}", verg_dir.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn init_creates_directory_structure() {
        let dir = TempDir::new().unwrap();
        run(dir.path()).unwrap();

        assert!(dir.path().join("verg/hosts.toml").exists());
        assert!(dir.path().join("verg/state/base.toml").exists());
        assert!(dir.path().join("verg/groups").is_dir());
        assert!(dir.path().join("verg/files").is_dir());
        assert!(dir.path().join("verg/templates").is_dir());
    }
}
