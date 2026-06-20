use std::path::Path;

use crate::error::Error;

/// Initialize a verg project at the given path.
/// The path IS the verg project directory (not a parent).
pub fn run(path: &Path, force: bool) -> Result<(), Error> {
    for sub in ["state", "groups", "files", "templates"] {
        std::fs::create_dir_all(path.join(sub))
            .map_err(|e| Error::Config(format!("failed to create {sub} dir: {e}")))?;
    }

    let hosts = "# Static host inventory\n# [hosts.example]\n# address = \"192.0.2.10\"\n# user = \"root\"\n# groups = [\"web\", \"prod\"]\n";
    write_scaffold(&path.join("hosts.toml"), hosts, force)?;

    let base_state = "# Base state applied to all hosts\n# [resource.pkg.essential]\n# names = [\"curl\", \"htop\"]\n# state = \"present\"\n";
    write_scaffold(&path.join("state").join("base.toml"), base_state, force)?;

    eprintln!("Initialized verg project at {}", path.display());
    eprintln!("Next steps:");
    eprintln!(
        "  1. Edit {}/hosts.toml to add your servers",
        path.display()
    );
    eprintln!(
        "  2. Edit {}/state/base.toml to declare desired state",
        path.display()
    );
    eprintln!("  3. Preview with: verg diff -t all");
    Ok(())
}

/// Write a scaffold file, skipping (with a notice) if it exists unless `force`.
fn write_scaffold(path: &Path, content: &str, force: bool) -> Result<(), Error> {
    if !force && path.exists() {
        eprintln!(
            "  {} exists, leaving it unchanged (use --force to overwrite)",
            path.display()
        );
        return Ok(());
    }
    std::fs::write(path, content)
        .map_err(|e| Error::Config(format!("failed to write {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn init_creates_directory_structure() {
        let dir = TempDir::new().unwrap();
        let verg_dir = dir.path().join("verg");
        run(&verg_dir, false).unwrap();

        assert!(verg_dir.join("hosts.toml").exists());
        assert!(verg_dir.join("state/base.toml").exists());
        assert!(verg_dir.join("groups").is_dir());
        assert!(verg_dir.join("files").is_dir());
        assert!(verg_dir.join("templates").is_dir());
    }

    #[test]
    fn init_does_not_clobber_existing_files() {
        let dir = TempDir::new().unwrap();
        let verg = dir.path().join("verg");
        run(&verg, false).unwrap();
        // User edits hosts.toml.
        std::fs::write(
            verg.join("hosts.toml"),
            "[hosts.real]\naddress = \"192.0.2.5\"\n",
        )
        .unwrap();
        // Re-running init must NOT wipe it.
        run(&verg, false).unwrap();
        let content = std::fs::read_to_string(verg.join("hosts.toml")).unwrap();
        assert!(
            content.contains("hosts.real"),
            "init clobbered the user's hosts.toml"
        );
    }

    #[test]
    fn init_force_overwrites() {
        let dir = TempDir::new().unwrap();
        let verg = dir.path().join("verg");
        run(&verg, false).unwrap();
        std::fs::write(verg.join("hosts.toml"), "custom").unwrap();
        run(&verg, true).unwrap();
        let content = std::fs::read_to_string(verg.join("hosts.toml")).unwrap();
        assert!(!content.contains("custom"), "force should overwrite");
    }
}
