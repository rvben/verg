use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::resource_def::{ParamSpec, ResourceDef, validate_param_specs};

/// A native provider definition as it travels in the bundle: `source` is the
/// embedded script TEXT (already read from disk on the control host).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProviderDef {
    #[serde(default)]
    pub description: String,
    pub interpreter: Vec<String>,
    pub source: String,
    #[serde(default)]
    pub params: HashMap<String, ParamSpec>,
}

/// A provider entry as written in `providers/*.toml`: `source` is a PATH
/// (resolved against the project directory and read into `ProviderDef.source`).
#[derive(Debug, Deserialize)]
struct ProviderConfig {
    #[serde(default)]
    description: String,
    interpreter: Vec<String>,
    source: String,
    #[serde(default)]
    params: HashMap<String, ParamSpec>,
}

#[derive(Debug, Deserialize)]
struct ProviderFile {
    #[serde(default)]
    provider: HashMap<String, ProviderConfig>,
}

/// Load provider definitions from `dir` (`providers/*.toml`). Each provider's
/// `source` path is resolved against `base_dir` and its contents embedded.
/// Type names must not collide with built-in types or declarative resource_defs.
pub fn load_provider_defs(
    dir: &Path,
    base_dir: &Path,
    builtin_types: &[&str],
    custom_def_types: &HashMap<String, ResourceDef>,
) -> Result<HashMap<String, ProviderDef>, Error> {
    if !dir.exists() {
        return Ok(HashMap::new());
    }

    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| Error::Config(format!("failed to read {}: {e}", dir.display())))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| Error::Config(format!("failed to read entry in {}: {e}", dir.display())))?
        .into_iter()
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "toml"))
        .collect();
    entries.sort_by_key(|e| e.path());

    let mut result: HashMap<String, ProviderDef> = HashMap::new();
    let mut seen_types: HashMap<String, String> = HashMap::new();

    for entry in entries {
        let path = entry.path();
        let file_path = path.display().to_string();
        let content = std::fs::read_to_string(&path)
            .map_err(|e| Error::Config(format!("failed to read {file_path}: {e}")))?;
        let parsed: ProviderFile = toml::from_str(&content)
            .map_err(|e| Error::Config(format!("failed to parse {file_path}: {e}")))?;

        for (type_name, cfg) in parsed.provider {
            if builtin_types.contains(&type_name.as_str()) {
                return Err(Error::Config(format!(
                    "provider type '{type_name}' in {file_path} conflicts with built-in resource type"
                )));
            }
            if custom_def_types.contains_key(&type_name) {
                return Err(Error::Config(format!(
                    "provider type '{type_name}' in {file_path} conflicts with a custom resource_def of the same name"
                )));
            }
            if let Some(prev_file) = seen_types.get(&type_name) {
                return Err(Error::Config(format!(
                    "duplicate provider type '{type_name}': first defined in {prev_file}, also in {file_path}"
                )));
            }
            if cfg.interpreter.is_empty() {
                return Err(Error::Config(format!(
                    "provider '{type_name}' in {file_path}: interpreter must be a non-empty argv array"
                )));
            }

            validate_param_specs(&type_name, &cfg.params, &file_path)?;

            let source_path = base_dir.join(&cfg.source);
            let source = std::fs::read_to_string(&source_path).map_err(|e| {
                Error::Config(format!(
                    "provider '{type_name}' in {file_path}: failed to read source '{}': {e}",
                    cfg.source
                ))
            })?;

            seen_types.insert(type_name.clone(), file_path.clone());
            result.insert(
                type_name,
                ProviderDef {
                    description: cfg.description,
                    interpreter: cfg.interpreter,
                    source,
                    params: cfg.params,
                },
            );
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn write(dir: &TempDir, rel: &str, content: &str) {
        let path = dir.path().join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn loads_provider_and_embeds_source_text() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "providers/dns.toml",
            r#"
[provider.dns_record]
description = "Manage a DNS record"
interpreter = ["python3"]
source = "providers/dns_record.py"

[provider.dns_record.params.zone]
type = "string"
required = true
"#,
        );
        write(
            &dir,
            "providers/dns_record.py",
            "import sys\nprint('{\"status\":\"ok\"}')\n",
        );

        let defs = load_provider_defs(
            &dir.path().join("providers"),
            dir.path(),
            &["file", "cmd"],
            &HashMap::new(),
        )
        .unwrap();

        let def = defs.get("dns_record").expect("dns_record present");
        assert_eq!(def.interpreter, vec!["python3"]);
        assert_eq!(def.description, "Manage a DNS record");
        // source is the EMBEDDED FILE TEXT, not the path.
        assert!(
            def.source.contains("import sys"),
            "source must be embedded text"
        );
        assert!(def.params.contains_key("zone"));
    }

    #[test]
    fn missing_dir_returns_empty() {
        let dir = TempDir::new().unwrap();
        let defs =
            load_provider_defs(&dir.path().join("nope"), dir.path(), &[], &HashMap::new()).unwrap();
        assert!(defs.is_empty());
    }

    #[test]
    fn missing_source_file_is_config_error() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "providers/p.toml",
            r#"
[provider.thing]
interpreter = ["/bin/sh"]
source = "providers/does_not_exist.sh"
"#,
        );
        let err = load_provider_defs(
            &dir.path().join("providers"),
            dir.path(),
            &[],
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("does_not_exist.sh"), "got: {err}");
    }

    #[test]
    fn empty_interpreter_is_config_error() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "providers/p.toml",
            r#"
[provider.thing]
interpreter = []
source = "providers/x.sh"
"#,
        );
        write(&dir, "providers/x.sh", "echo hi\n");
        let err = load_provider_defs(
            &dir.path().join("providers"),
            dir.path(),
            &[],
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("interpreter"), "got: {err}");
    }

    #[test]
    fn collision_with_builtin_is_config_error() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "providers/p.toml",
            r#"
[provider.file]
interpreter = ["/bin/sh"]
source = "providers/x.sh"
"#,
        );
        write(&dir, "providers/x.sh", "echo hi\n");
        let err = load_provider_defs(
            &dir.path().join("providers"),
            dir.path(),
            &["file", "cmd"],
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("built-in"), "got: {err}");
    }

    #[test]
    fn collision_with_resource_def_is_config_error() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "providers/p.toml",
            r#"
[provider.deploy]
interpreter = ["/bin/sh"]
source = "providers/x.sh"
"#,
        );
        write(&dir, "providers/x.sh", "echo hi\n");
        let mut custom: HashMap<String, crate::resource_def::ResourceDef> = HashMap::new();
        custom.insert(
            "deploy".to_string(),
            crate::resource_def::ResourceDef {
                description: String::new(),
                params: HashMap::new(),
                check: "true".into(),
                apply: "true".into(),
            },
        );
        let err = load_provider_defs(&dir.path().join("providers"), dir.path(), &[], &custom)
            .unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("deploy"), "got: {err}");
        assert!(
            err.to_string().contains("resource_def") || err.to_string().contains("custom"),
            "got: {err}"
        );
    }

    #[test]
    fn invalid_param_is_config_error() {
        let dir = TempDir::new().unwrap();
        write(
            &dir,
            "providers/p.toml",
            r#"
[provider.thing]
interpreter = ["/bin/sh"]
source = "providers/x.sh"

[provider.thing.params.source]
type = "string"
"#,
        );
        write(&dir, "providers/x.sh", "echo hi\n");
        let err = load_provider_defs(
            &dir.path().join("providers"),
            dir.path(),
            &[],
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("reserved"), "got: {err}");
    }

    #[test]
    fn ignores_non_toml_files() {
        let dir = TempDir::new().unwrap();
        write(&dir, "providers/notes.txt", "ignore me");
        write(
            &dir,
            "providers/p.toml",
            r#"
[provider.thing]
interpreter = ["/bin/sh"]
source = "providers/x.sh"
"#,
        );
        write(&dir, "providers/x.sh", "echo hi\n");
        let defs = load_provider_defs(
            &dir.path().join("providers"),
            dir.path(),
            &[],
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(defs.len(), 1);
    }
}
