pub mod groups;
pub mod selector;
pub mod static_hosts;

use std::collections::HashMap;
use std::path::Path;

use crate::error::Error;

#[derive(Debug, Clone)]
pub struct Host {
    pub name: String,
    pub address: String,
    pub user: String,
    pub groups: Vec<String>,
    pub vars: HashMap<String, toml::Value>,
}

#[derive(Debug)]
pub struct Inventory {
    pub hosts: HashMap<String, Host>,
}

impl Inventory {
    pub fn load(base_dir: &Path) -> Result<Self, Error> {
        let hosts_path = base_dir.join("hosts.toml");
        let groups_dir = base_dir.join("groups");

        let host_defs = if hosts_path.exists() {
            static_hosts::load_hosts(&hosts_path)?
        } else {
            HashMap::new()
        };

        let group_defs = groups::load_groups(&groups_dir)?;

        let mut hosts = HashMap::new();
        for (name, def) in host_defs {
            let mut vars = def.vars.clone();
            // Merge group vars (host vars take precedence)
            for group_name in &def.groups {
                if let Some(group) = group_defs.get(group_name) {
                    for (k, v) in &group.vars {
                        vars.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
            }
            hosts.insert(
                name.clone(),
                Host {
                    name,
                    address: def.address,
                    user: def.user,
                    groups: def.groups,
                    vars,
                },
            );
        }

        Ok(Inventory { hosts })
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::TempDir;

    use super::*;

    fn setup_inventory(dir: &TempDir) {
        std::fs::write(
            dir.path().join("hosts.toml"),
            r#"
[hosts.web1]
address = "192.168.1.10"
groups = ["web"]

[hosts.web1.vars]
custom = "override"
"#,
        )
        .unwrap();

        let groups_dir = dir.path().join("groups");
        std::fs::create_dir(&groups_dir).unwrap();
        let mut f = std::fs::File::create(groups_dir.join("web.toml")).unwrap();
        write!(
            f,
            r#"
[vars]
http_port = 80
custom = "from_group"
"#
        )
        .unwrap();
    }

    #[test]
    fn load_inventory_merges_group_vars() {
        let dir = TempDir::new().unwrap();
        setup_inventory(&dir);

        let inv = Inventory::load(dir.path()).unwrap();
        let host = &inv.hosts["web1"];
        assert_eq!(host.vars["http_port"], toml::Value::Integer(80));
        // Host var takes precedence over group var
        assert_eq!(host.vars["custom"], toml::Value::String("override".into()));
    }

    #[test]
    fn empty_directory_returns_empty_inventory() {
        let dir = TempDir::new().unwrap();
        let inv = Inventory::load(dir.path()).unwrap();
        assert!(inv.hosts.is_empty());
    }
}
