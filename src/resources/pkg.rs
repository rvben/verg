use crate::error::Error;

use super::{ResolvedResource, ResourceResult, ResourceStatus, run_cmd};

#[derive(Debug)]
enum PkgManager {
    Apt,
    Dnf,
    Pacman,
}

fn detect_pkg_manager() -> Result<PkgManager, Error> {
    if run_cmd("which", &["apt-get"]).is_ok_and(|o| o.status.success()) {
        return Ok(PkgManager::Apt);
    }
    if run_cmd("which", &["dnf"]).is_ok_and(|o| o.status.success()) {
        return Ok(PkgManager::Dnf);
    }
    if run_cmd("which", &["pacman"]).is_ok_and(|o| o.status.success()) {
        return Ok(PkgManager::Pacman);
    }
    Err(Error::Resource("no supported package manager found".into()))
}

/// True only when `dpkg -s` reports a fully-installed package. `dpkg -s`
/// exits 0 even for `deinstall ok config-files` (removed, config retained),
/// so the exit code alone is not sufficient.
fn dpkg_reports_installed(status_output: &str) -> bool {
    status_output.lines().any(|line| {
        let line = line.trim();
        line.starts_with("Status:") && line.contains("install ok installed")
    })
}

fn is_installed(mgr: &PkgManager, name: &str) -> Result<bool, Error> {
    match mgr {
        PkgManager::Apt => {
            // Two-level: dpkg -s must exit 0 AND report "install ok installed" (it exits 0 for config-files state too).
            let output = run_cmd("dpkg", &["-s", name])?;
            Ok(output.status.success()
                && dpkg_reports_installed(&String::from_utf8_lossy(&output.stdout)))
        }
        PkgManager::Dnf => Ok(run_cmd("rpm", &["-q", name])?.status.success()),
        PkgManager::Pacman => Ok(run_cmd("pacman", &["-Qi", name])?.status.success()),
    }
}

fn update_cache(mgr: &PkgManager) -> Result<(), Error> {
    let output = match mgr {
        PkgManager::Apt => run_cmd("apt-get", &["update", "-qq"])?,
        PkgManager::Dnf => run_cmd("dnf", &["makecache", "-q"])?,
        PkgManager::Pacman => run_cmd("pacman", &["-Sy"])?,
    };
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(Error::Resource(format!(
            "failed to update package cache: {stderr}"
        )))
    }
}

fn install(mgr: &PkgManager, name: &str) -> Result<(), Error> {
    let output = match mgr {
        PkgManager::Apt => run_cmd("apt-get", &["install", "-y", name])?,
        PkgManager::Dnf => run_cmd("dnf", &["install", "-y", name])?,
        PkgManager::Pacman => run_cmd("pacman", &["-S", "--noconfirm", name])?,
    };
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(Error::Resource(format!(
            "failed to install {name}: {stderr}"
        )))
    }
}

fn remove(mgr: &PkgManager, name: &str) -> Result<(), Error> {
    let output = match mgr {
        PkgManager::Apt => run_cmd("apt-get", &["remove", "-y", name])?,
        PkgManager::Dnf => run_cmd("dnf", &["remove", "-y", name])?,
        PkgManager::Pacman => run_cmd("pacman", &["-R", "--noconfirm", name])?,
    };
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(Error::Resource(format!(
            "failed to remove {name}: {stderr}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpkg_installed_status_is_recognized() {
        let installed = "Package: nginx\nStatus: install ok installed\nVersion: 1.0\n";
        assert!(dpkg_reports_installed(installed));
    }

    #[test]
    fn dpkg_config_files_status_is_not_installed() {
        // `apt-get remove` (not purge) leaves this state; binaries are gone.
        let config_files = "Package: nginx\nStatus: deinstall ok config-files\nVersion: 1.0\n";
        assert!(!dpkg_reports_installed(config_files));
    }

    #[test]
    fn dpkg_empty_output_is_not_installed() {
        assert!(!dpkg_reports_installed(""));
    }
}

pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let mgr = detect_pkg_manager()?;
    let state = resource
        .props
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("present");

    let names: Vec<String> = if let Some(toml::Value::Array(arr)) = resource.props.get("names") {
        arr.iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    } else if let Some(name) = resource.props.get("name").and_then(|v| v.as_str()) {
        vec![name.to_string()]
    } else {
        return Err(Error::Resource(
            "pkg resource requires 'name' or 'names'".into(),
        ));
    };

    let mut any_changed = false;
    let mut changes = Vec::new();
    let mut cache_updated = false;

    for name in &names {
        let installed = is_installed(&mgr, name)?;
        match (state, installed) {
            ("present", false) => {
                if dry_run {
                    changes.push(format!("+{name}"));
                } else {
                    if !cache_updated {
                        update_cache(&mgr)?;
                        cache_updated = true;
                    }
                    install(&mgr, name)?;
                }
                any_changed = true;
            }
            ("absent", true) => {
                if dry_run {
                    changes.push(format!("-{name}"));
                } else {
                    remove(&mgr, name)?;
                }
                any_changed = true;
            }
            _ => {}
        }
    }

    Ok(ResourceResult {
        resource_type: "pkg".into(),
        name: resource.name.clone(),
        status: if any_changed {
            ResourceStatus::Changed
        } else {
            ResourceStatus::Ok
        },
        diff: if changes.is_empty() {
            None
        } else {
            Some(changes.join(", "))
        },
        from: None,
        to: None,
        error: None,
        output: None,
    })
}
