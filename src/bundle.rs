use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::inventory::Host;
use crate::resources::ResolvedResource;
use crate::state::StateFile;
use crate::state::vars;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub host: String,
    pub resources: Vec<ResolvedResource>,
}

impl Bundle {
    /// Build a bundle for a specific host.
    /// `base_dir` is the verg project directory (used to resolve `source` file paths).
    pub fn build(host: &Host, state_files: &[StateFile], base_dir: &Path) -> Result<Self, Error> {
        let mut resources = Vec::new();

        for sf in state_files {
            if let Some(targets) = &sf.targets {
                let applies = targets
                    .iter()
                    .any(|t| host.groups.contains(t) || host.name == *t);
                if !applies {
                    continue;
                }
            }

            for decl in sf.resources()? {
                let mut props = HashMap::new();
                let mut after = Vec::new();
                let mut notify = Vec::new();

                for (key, value) in &decl.props {
                    if key == "after" {
                        if let toml::Value::Array(arr) = value {
                            for item in arr {
                                if let toml::Value::String(s) = item {
                                    after.push(s.clone());
                                }
                            }
                        }
                    } else if key == "notify" {
                        match value {
                            toml::Value::String(s) => notify.push(s.clone()),
                            toml::Value::Array(arr) => {
                                for item in arr {
                                    if let toml::Value::String(s) = item {
                                        notify.push(s.clone());
                                    }
                                }
                            }
                            _ => {}
                        }
                    } else {
                        let interpolated = match value {
                            toml::Value::String(s) => {
                                toml::Value::String(vars::interpolate(s, &host.vars)?)
                            }
                            other => other.clone(),
                        };
                        props.insert(key.clone(), interpolated);
                    }
                }

                if let Some(toml::Value::Table(var_overrides)) = props.remove("vars") {
                    for (k, v) in var_overrides {
                        if let toml::Value::String(s) = &v {
                            let interpolated = vars::interpolate(s, &host.vars)?;
                            props.entry(k).or_insert(toml::Value::String(interpolated));
                        }
                    }
                }

                // Resolve `source` files on the control machine and inline as `content`
                if let Some(toml::Value::String(source_path)) = props.remove("source") {
                    let full_path = base_dir.join(&source_path);
                    let content = std::fs::read_to_string(&full_path).map_err(|e| {
                        Error::Config(format!(
                            "failed to read source file {}: {e}",
                            full_path.display()
                        ))
                    })?;
                    props.insert("content".into(), toml::Value::String(content));
                }

                resources.push(ResolvedResource {
                    resource_type: decl.resource_type,
                    name: decl.name,
                    props,
                    after,
                    notify,
                });
            }
        }

        Ok(Bundle {
            host: host.name.clone(),
            resources,
        })
    }

    pub fn to_toml(&self) -> Result<String, Error> {
        toml::to_string_pretty(self)
            .map_err(|e| Error::Other(format!("failed to serialize bundle: {e}")))
    }

    pub fn from_toml(input: &str) -> Result<Self, Error> {
        toml::from_str(input).map_err(|e| Error::Parse(format!("failed to parse bundle: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_host() -> Host {
        Host {
            name: "web1".into(),
            address: "192.168.1.10".into(),
            user: "root".into(),
            port: None,
            groups: vec!["web".into(), "prod".into()],
            vars: {
                let mut v = HashMap::new();
                v.insert("http_port".into(), toml::Value::Integer(80));
                v.insert(
                    "document_root".into(),
                    toml::Value::String("/var/www".into()),
                );
                v
            },
        }
    }

    fn parse_state(toml_str: &str) -> StateFile {
        toml::from_str(toml_str).unwrap()
    }

    #[test]
    fn bundle_includes_matching_state_files() {
        let host = test_host();
        let files = vec![
            parse_state(
                r#"
[resource.pkg.curl]
name = "curl"
state = "present"
"#,
            ),
            parse_state(
                r#"
targets = ["web"]

[resource.pkg.nginx]
name = "nginx"
state = "present"
"#,
            ),
            parse_state(
                r#"
targets = ["db"]

[resource.pkg.postgres]
name = "postgresql"
state = "present"
"#,
            ),
        ];

        let bundle = Bundle::build(&host, &files, Path::new("/tmp")).unwrap();
        assert_eq!(bundle.resources.len(), 2);
    }

    #[test]
    fn bundle_interpolates_variables() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
targets = ["web"]

[resource.file.conf]
path = "/etc/nginx/nginx.conf"
content = "listen {{ http_port }}"
"#,
        )];

        let bundle = Bundle::build(&host, &files, Path::new("/tmp")).unwrap();
        assert_eq!(
            bundle.resources[0].props["content"],
            toml::Value::String("listen 80".into())
        );
    }

    #[test]
    fn bundle_extracts_after_dependencies() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.service.nginx]
name = "nginx"
state = "running"
after = ["pkg.nginx", "file.conf"]
"#,
        )];

        let bundle = Bundle::build(&host, &files, Path::new("/tmp")).unwrap();
        assert_eq!(bundle.resources[0].after, vec!["pkg.nginx", "file.conf"]);
        assert!(!bundle.resources[0].props.contains_key("after"));
    }

    #[test]
    fn bundle_roundtrip_toml() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.pkg.nginx]
name = "nginx"
state = "present"
"#,
        )];

        let bundle = Bundle::build(&host, &files, Path::new("/tmp")).unwrap();
        let serialized = bundle.to_toml().unwrap();
        let deserialized = Bundle::from_toml(&serialized).unwrap();
        assert_eq!(deserialized.host, "web1");
        assert_eq!(deserialized.resources.len(), 1);
        assert_eq!(deserialized.resources[0].fqn(), "pkg.nginx");
    }

    #[test]
    fn bundle_resolves_source_to_content() {
        let host = test_host();
        let dir = tempfile::TempDir::new().unwrap();
        let files_dir = dir.path().join("files");
        std::fs::create_dir(&files_dir).unwrap();
        std::fs::write(files_dir.join("test.conf"), "server_name web1;").unwrap();

        let files = vec![parse_state(
            r#"
[resource.file.conf]
path = "/etc/test.conf"
source = "files/test.conf"
"#,
        )];

        let bundle = Bundle::build(&host, &files, dir.path()).unwrap();
        assert_eq!(
            bundle.resources[0].props["content"],
            toml::Value::String("server_name web1;".into())
        );
        assert!(!bundle.resources[0].props.contains_key("source"));
    }

    #[test]
    fn undefined_variable_errors() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.file.conf]
content = "{{ undefined_var }}"
"#,
        )];

        let result = Bundle::build(&host, &files, Path::new("/tmp"));
        assert!(matches!(result, Err(Error::Parse(_))));
    }
}
