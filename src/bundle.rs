use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::inventory::Host;
use crate::resources::{REGISTER_SENTINEL, REGISTER_SENTINEL_END, ResolvedResource};
use crate::state::StateFile;
use crate::state::vars;

fn protect_register_refs(input: &str) -> (String, Vec<String>) {
    let mut result = String::with_capacity(input.len());
    let mut names = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '{' && chars.peek() == Some(&'{') {
            chars.next(); // consume second {
            let mut inner = String::new();
            let mut found_close = false;
            while let Some(ch) = chars.next() {
                if ch == '}' && chars.peek() == Some(&'}') {
                    chars.next();
                    found_close = true;
                    break;
                }
                inner.push(ch);
            }
            if found_close {
                let trimmed = inner.trim();
                if let Some(reg_name) = trimmed.strip_prefix("register.") {
                    let reg_name = reg_name.trim().to_string();
                    result.push_str(REGISTER_SENTINEL);
                    result.push_str(&reg_name);
                    result.push_str(REGISTER_SENTINEL_END);
                    if !names.contains(&reg_name) {
                        names.push(reg_name);
                    }
                } else {
                    result.push_str("{{");
                    result.push_str(&inner);
                    result.push_str("}}");
                }
            } else {
                result.push_str("{{");
                result.push_str(&inner);
            }
        } else {
            result.push(c);
        }
    }

    (result, names)
}

fn restore_register_refs(input: &str) -> String {
    let mut result = input.to_string();
    while let Some(start) = result.find(REGISTER_SENTINEL) {
        let after_prefix = start + REGISTER_SENTINEL.len();
        if let Some(end) = result[after_prefix..].find(REGISTER_SENTINEL_END) {
            let name = result[after_prefix..after_prefix + end].to_string();
            let sentinel = format!("{REGISTER_SENTINEL}{name}{REGISTER_SENTINEL_END}");
            let replacement = format!("{{{{ register.{name} }}}}");
            result = result.replacen(&sentinel, &replacement, 1);
        } else {
            break;
        }
    }
    result
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub host: String,
    pub resources: Vec<ResolvedResource>,
    #[serde(default)]
    pub facts: HashMap<String, String>,
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
                let mut when = None;
                let mut handler = false;
                let mut register = None;

                for (key, value) in &decl.props {
                    if key == "when" {
                        if let toml::Value::String(s) = value {
                            when = Some(s.clone());
                        }
                    } else if key == "after" {
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
                    } else if key == "handler" {
                        if let toml::Value::Boolean(b) = value {
                            handler = *b;
                        }
                    } else if key == "register" {
                        if let toml::Value::String(s) = value {
                            register = Some(s.clone());
                        }
                    } else {
                        let interpolated = match value {
                            toml::Value::String(s) => {
                                let (protected, _) = protect_register_refs(s);
                                let rendered = vars::render(&protected, &host.vars)?;
                                toml::Value::String(restore_register_refs(&rendered))
                            }
                            other => other.clone(),
                        };
                        props.insert(key.clone(), interpolated);
                    }
                }

                if let Some(toml::Value::Table(var_overrides)) = props.remove("vars") {
                    for (k, v) in var_overrides {
                        if let toml::Value::String(s) = &v {
                            let interpolated = vars::render(s, &host.vars)?;
                            props.entry(k).or_insert(toml::Value::String(interpolated));
                        }
                    }
                }

                let is_template = props
                    .remove("template")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                // Resolve `source` files on the control machine and inline as `content`
                if let Some(toml::Value::String(source_path)) = props.remove("source") {
                    let full_path = base_dir.join(&source_path);
                    let content = std::fs::read_to_string(&full_path).map_err(|e| {
                        Error::Config(format!(
                            "failed to read source file {}: {e}",
                            full_path.display()
                        ))
                    })?;
                    let content = if is_template {
                        let (protected, _) = protect_register_refs(&content);
                        let rendered = vars::render(&protected, &host.vars).map_err(|e| {
                            Error::Config(format!(
                                "{}.{}: template error in source {}: {e}",
                                decl.resource_type, decl.name, source_path
                            ))
                        })?;
                        restore_register_refs(&rendered)
                    } else {
                        content
                    };
                    props.insert("content".into(), toml::Value::String(content));
                }

                // Resolve `compose_file` for docker_compose resources
                if let Some(toml::Value::String(compose_path)) = props.remove("compose_file") {
                    let full_path = base_dir.join(&compose_path);
                    let content = std::fs::read_to_string(&full_path).map_err(|e| {
                        Error::Config(format!(
                            "failed to read compose file {}: {e}",
                            full_path.display()
                        ))
                    })?;
                    let content = if is_template {
                        let (protected, _) = protect_register_refs(&content);
                        let rendered = vars::render(&protected, &host.vars).map_err(|e| {
                            Error::Config(format!(
                                "{}.{}: template error in compose file {}: {e}",
                                decl.resource_type, decl.name, compose_path
                            ))
                        })?;
                        restore_register_refs(&rendered)
                    } else {
                        content
                    };
                    props.insert("content".into(), toml::Value::String(content));
                }

                // Resolve `env_file` for docker_compose resources
                if let Some(toml::Value::String(env_path)) = props.remove("env_file") {
                    let full_path = base_dir.join(&env_path);
                    let content = std::fs::read_to_string(&full_path).map_err(|e| {
                        Error::Config(format!(
                            "failed to read env file {}: {e}",
                            full_path.display()
                        ))
                    })?;
                    props.insert("env_content".into(), toml::Value::String(content));
                }

                resources.push(ResolvedResource {
                    resource_type: decl.resource_type,
                    name: decl.name,
                    props,
                    after,
                    notify,
                    when,
                    handler,
                    register,
                });
            }
        }

        // Validate register names are unique
        let mut register_names: HashMap<String, String> = HashMap::new();
        for r in &resources {
            if let Some(ref reg_name) = r.register {
                if let Some(existing_fqn) = register_names.get(reg_name) {
                    return Err(Error::Config(format!(
                        "duplicate register name '{reg_name}': used by both {existing_fqn} and {}",
                        r.fqn()
                    )));
                }
                register_names.insert(reg_name.clone(), r.fqn());
            }
        }

        // Validate register references have proper after dependencies
        for r in &resources {
            for value in r.props.values() {
                if let toml::Value::String(s) = value {
                    let (_, ref_names) = protect_register_refs(s);
                    for ref_name in ref_names {
                        let reg_fqn = register_names.get(&ref_name).ok_or_else(|| {
                            Error::Config(format!(
                                "{}: references unknown register '{ref_name}'",
                                r.fqn()
                            ))
                        })?;
                        if !r.after.contains(reg_fqn) {
                            return Err(Error::Config(format!(
                                "{}: uses register '{ref_name}' but does not declare after = [\"{reg_fqn}\"]",
                                r.fqn()
                            )));
                        }
                    }
                }
            }
        }

        // Extract fact.* and group.* vars into the facts map for when evaluation
        let mut facts = HashMap::new();
        for (k, v) in &host.vars {
            if (k.starts_with("fact.") || k.starts_with("group."))
                && let toml::Value::String(s) = v
            {
                facts.insert(k.clone(), s.clone());
            }
        }

        Ok(Bundle {
            host: host.name.clone(),
            resources,
            facts,
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

    #[test]
    fn bundle_renders_template_source_file() {
        let host = test_host();
        let dir = tempfile::TempDir::new().unwrap();
        let files_dir = dir.path().join("files");
        std::fs::create_dir(&files_dir).unwrap();
        std::fs::write(
            files_dir.join("test.conf.j2"),
            "listen {{ http_port }}\nroot {{ document_root }}",
        )
        .unwrap();

        let files = vec![parse_state(
            r#"
[resource.file.conf]
path = "/etc/test.conf"
source = "files/test.conf.j2"
template = true
"#,
        )];

        let bundle = Bundle::build(&host, &files, dir.path()).unwrap();
        assert_eq!(
            bundle.resources[0].props["content"],
            toml::Value::String("listen 80\nroot /var/www".into())
        );
    }

    #[test]
    fn bundle_does_not_render_source_without_template_flag() {
        let host = test_host();
        let dir = tempfile::TempDir::new().unwrap();
        let files_dir = dir.path().join("files");
        std::fs::create_dir(&files_dir).unwrap();
        std::fs::write(files_dir.join("raw.conf"), "{{ not_rendered }}").unwrap();

        let files = vec![parse_state(
            r#"
[resource.file.raw]
path = "/etc/raw.conf"
source = "files/raw.conf"
"#,
        )];

        let bundle = Bundle::build(&host, &files, dir.path()).unwrap();
        assert_eq!(
            bundle.resources[0].props["content"],
            toml::Value::String("{{ not_rendered }}".into())
        );
    }

    #[test]
    fn bundle_extracts_handler_flag() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.cmd.nginx-reload]
command = "nginx -t && systemctl reload nginx"
handler = true
unless = "true"
"#,
        )];

        let bundle = Bundle::build(&host, &files, Path::new("/tmp")).unwrap();
        assert!(bundle.resources[0].handler);
        assert!(!bundle.resources[0].props.contains_key("handler"));
    }

    #[test]
    fn bundle_passes_through_register_references() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.cmd.get-ip]
command = "hostname -I"
register = "host_ip"

[resource.file.conf]
path = "/etc/app.conf"
content = "ip={{ register.host_ip }}"
after = ["cmd.get-ip"]
"#,
        )];

        let bundle = Bundle::build(&host, &files, Path::new("/tmp")).unwrap();
        let content = bundle.resources.iter().find(|r| r.name == "conf").unwrap();
        let val = content.props["content"].as_str().unwrap();
        assert!(
            val.contains("register.host_ip"),
            "register ref should survive: {val}"
        );
    }

    #[test]
    fn bundle_errors_on_register_ref_without_dependency() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.cmd.get-ip]
command = "hostname -I"
register = "host_ip"

[resource.file.conf]
path = "/etc/app.conf"
content = "ip={{ register.host_ip }}"
"#,
        )];

        let result = Bundle::build(&host, &files, Path::new("/tmp"));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("after"));
    }

    #[test]
    fn bundle_errors_on_unknown_register_ref() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.file.conf]
path = "/etc/app.conf"
content = "ip={{ register.nonexistent }}"
"#,
        )];

        let result = Bundle::build(&host, &files, Path::new("/tmp"));
        assert!(result.is_err());
    }

    #[test]
    fn bundle_errors_on_duplicate_register_names() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.cmd.a]
command = "echo a"
register = "result"

[resource.cmd.b]
command = "echo b"
register = "result"
"#,
        )];

        let result = Bundle::build(&host, &files, Path::new("/tmp"));
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("result"));
    }

    #[test]
    fn bundle_extracts_register_field() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.cmd.get-ip]
command = "hostname -I"
register = "host_ip"
"#,
        )];

        let bundle = Bundle::build(&host, &files, Path::new("/tmp")).unwrap();
        assert_eq!(bundle.resources[0].register, Some("host_ip".into()));
        assert!(!bundle.resources[0].props.contains_key("register"));
    }
}
