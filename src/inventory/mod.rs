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

    pub fn filter(&self, selector: &selector::Selector) -> Result<Vec<&Host>, Error> {
        use selector::Selector;
        match selector {
            Selector::All => Ok(self.hosts.values().collect()),
            Selector::Group(name) => {
                if let Some(host) = self.hosts.get(name) {
                    return Ok(vec![host]);
                }
                let matches: Vec<_> = self
                    .hosts
                    .values()
                    .filter(|h| h.groups.contains(name))
                    .collect();
                if matches.is_empty() {
                    return Err(Error::TargetNotFound(name.clone()));
                }
                Ok(matches)
            }
            Selector::Exclude(inner) => {
                let excluded = self.filter(inner)?;
                let excluded_names: std::collections::HashSet<_> =
                    excluded.iter().map(|h| &h.name).collect();
                Ok(self
                    .hosts
                    .values()
                    .filter(|h| !excluded_names.contains(&h.name))
                    .collect())
            }
            Selector::Union(selectors) => {
                let mut seen = std::collections::HashSet::new();
                let mut result = Vec::new();
                for sel in selectors {
                    for host in self.filter(sel)? {
                        if seen.insert(&host.name) {
                            result.push(host);
                        }
                    }
                }
                Ok(result)
            }
            Selector::Intersection(selectors) => {
                let sets: Vec<std::collections::HashSet<_>> = selectors
                    .iter()
                    .map(|sel| {
                        self.filter(sel)
                            .map(|hosts| hosts.iter().map(|h| h.name.clone()).collect())
                    })
                    .collect::<Result<_, _>>()?;
                if sets.is_empty() {
                    return Ok(vec![]);
                }
                let intersection = sets
                    .into_iter()
                    .reduce(|a, b| a.intersection(&b).cloned().collect())
                    .unwrap_or_default();
                Ok(self
                    .hosts
                    .values()
                    .filter(|h| intersection.contains(&h.name))
                    .collect())
            }
        }
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

    fn build_test_inventory() -> Inventory {
        let mut hosts = HashMap::new();
        hosts.insert(
            "web1".into(),
            Host {
                name: "web1".into(),
                address: "192.168.1.10".into(),
                user: "root".into(),
                groups: vec!["web".into(), "prod".into()],
                vars: HashMap::new(),
            },
        );
        hosts.insert(
            "web2".into(),
            Host {
                name: "web2".into(),
                address: "192.168.1.11".into(),
                user: "root".into(),
                groups: vec!["web".into(), "staging".into()],
                vars: HashMap::new(),
            },
        );
        hosts.insert(
            "db1".into(),
            Host {
                name: "db1".into(),
                address: "10.0.0.5".into(),
                user: "root".into(),
                groups: vec!["db".into(), "prod".into()],
                vars: HashMap::new(),
            },
        );
        Inventory { hosts }
    }

    #[test]
    fn filter_all() {
        let inv = build_test_inventory();
        let sel = selector::parse_selector("all").unwrap();
        let hosts = inv.filter(&sel).unwrap();
        assert_eq!(hosts.len(), 3);
    }

    #[test]
    fn filter_by_group() {
        let inv = build_test_inventory();
        let sel = selector::parse_selector("web").unwrap();
        let hosts = inv.filter(&sel).unwrap();
        assert_eq!(hosts.len(), 2);
    }

    #[test]
    fn filter_by_host_name() {
        let inv = build_test_inventory();
        let sel = selector::parse_selector("db1").unwrap();
        let hosts = inv.filter(&sel).unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "db1");
    }

    #[test]
    fn filter_intersection() {
        let inv = build_test_inventory();
        let sel = selector::parse_selector("prod:web").unwrap();
        let hosts = inv.filter(&sel).unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "web1");
    }

    #[test]
    fn filter_exclusion() {
        let inv = build_test_inventory();
        let sel = selector::parse_selector("prod:!web").unwrap();
        let hosts = inv.filter(&sel).unwrap();
        assert_eq!(hosts.len(), 1);
        assert_eq!(hosts[0].name, "db1");
    }

    #[test]
    fn filter_unknown_target_errors() {
        let inv = build_test_inventory();
        let sel = selector::parse_selector("nonexistent").unwrap();
        assert!(matches!(inv.filter(&sel), Err(Error::TargetNotFound(_))));
    }
}
