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

fn is_installed(mgr: &PkgManager, name: &str) -> Result<bool, Error> {
    let output = match mgr {
        PkgManager::Apt => run_cmd("dpkg", &["-s", name])?,
        PkgManager::Dnf => run_cmd("rpm", &["-q", name])?,
        PkgManager::Pacman => run_cmd("pacman", &["-Qi", name])?,
    };
    Ok(output.status.success())
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

    for name in &names {
        let installed = is_installed(&mgr, name)?;
        match (state, installed) {
            ("present", false) => {
                if dry_run {
                    changes.push(format!("+{name}"));
                } else {
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
    })
}
