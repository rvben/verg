pub mod vars;

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::Error;

#[derive(Debug, Clone, Deserialize)]
pub struct StateFile {
    #[serde(default)]
    pub targets: Option<Vec<String>>,
    #[serde(default)]
    pub resource: HashMap<String, HashMap<String, toml::Value>>,
}

#[derive(Debug, Clone)]
pub struct ResourceDecl {
    pub resource_type: String,
    pub name: String,
    pub props: toml::map::Map<String, toml::Value>,
}

impl StateFile {
    pub fn load(path: &Path) -> Result<Self, Error> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| Error::Config(format!("failed to read {}: {e}", path.display())))?;
        toml::from_str(&content)
            .map_err(|e| Error::Parse(format!("failed to parse {}: {e}", path.display())))
    }

    pub fn resources(&self) -> Result<Vec<ResourceDecl>, Error> {
        let mut decls = Vec::new();
        for (type_name, instances) in &self.resource {
            for (instance_name, value) in instances {
                let props = match value {
                    toml::Value::Table(t) => t.clone(),
                    _ => {
                        return Err(Error::Parse(format!(
                            "resource {type_name}.{instance_name} must be a table"
                        )));
                    }
                };
                decls.push(ResourceDecl {
                    resource_type: type_name.clone(),
                    name: instance_name.clone(),
                    props,
                });
            }
        }
        Ok(decls)
    }
}

pub fn load_state_dir(dir: &Path) -> Result<Vec<StateFile>, Error> {
    let mut files = Vec::new();
    if !dir.exists() {
        return Ok(files);
    }
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| Error::Config(format!("failed to read {}: {e}", dir.display())))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
        .collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        files.push(StateFile::load(&entry.path())?);
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::{NamedTempFile, TempDir};

    use super::*;

    #[test]
    fn parse_state_file() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"
targets = ["web"]

[resource.pkg.nginx]
name = "nginx"
state = "present"

[resource.file.nginx-conf]
path = "/etc/nginx/nginx.conf"
source = "files/nginx.conf"
after = ["pkg.nginx"]
"#
        )
        .unwrap();

        let state = StateFile::load(f.path()).unwrap();
        assert_eq!(state.targets, Some(vec!["web".to_string()]));
        let resources = state.resources().unwrap();
        assert_eq!(resources.len(), 2);
    }

    #[test]
    fn parse_state_file_no_targets() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"
[resource.pkg.essential]
names = ["curl", "htop"]
state = "present"
"#
        )
        .unwrap();

        let state = StateFile::load(f.path()).unwrap();
        assert!(state.targets.is_none());
    }

    #[test]
    fn load_state_directory() {
        let dir = TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("base.toml"),
            r#"
[resource.pkg.curl]
name = "curl"
state = "present"
"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("web.toml"),
            r#"
targets = ["web"]

[resource.pkg.nginx]
name = "nginx"
state = "present"
"#,
        )
        .unwrap();

        let files = load_state_dir(dir.path()).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files[0].targets.is_none());
        assert!(files[1].targets.is_some());
    }

    #[test]
    fn resource_decl_extraction() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"
[resource.service.nginx]
name = "nginx"
state = "running"
enabled = true
after = ["file.nginx-conf"]
"#
        )
        .unwrap();

        let state = StateFile::load(f.path()).unwrap();
        let resources = state.resources().unwrap();
        assert_eq!(resources.len(), 1);
        assert_eq!(resources[0].resource_type, "service");
        assert_eq!(resources[0].name, "nginx");
        assert_eq!(
            resources[0].props["state"],
            toml::Value::String("running".into())
        );
        assert_eq!(resources[0].props["enabled"], toml::Value::Boolean(true));
    }
}
