use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Error;

const RESERVED_PARAM_NAMES: &[&str] = &[
    "after",
    "notify",
    "when",
    "handler",
    "template",
    "register",
    "vars",
    "sensitive",
    "source",
    "compose_file",
    "env_file",
];

const ALLOWED_PARAM_TYPES: &[&str] = &["string", "integer", "float", "boolean"];

fn default_param_type() -> String {
    "string".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParamSpec {
    #[serde(rename = "type", default = "default_param_type")]
    pub param_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<toml::Value>,
    #[serde(rename = "enum", default)]
    pub enum_values: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceDef {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub params: HashMap<String, ParamSpec>,
    pub check: String,
    pub apply: String,
}

#[derive(Debug, Deserialize)]
struct DefFile {
    #[serde(default)]
    resource_def: HashMap<String, ResourceDef>,
}

fn is_env_safe_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn validate_resource_def(
    type_name: &str,
    def: &ResourceDef,
    builtin_types: &[&str],
    seen_types: &HashMap<String, String>,
    file_path: &str,
) -> Result<(), Error> {
    if builtin_types.contains(&type_name) {
        return Err(Error::Config(format!(
            "resource_def type '{type_name}' in {file_path} conflicts with built-in resource type"
        )));
    }

    if let Some(prev_file) = seen_types.get(type_name) {
        return Err(Error::Config(format!(
            "duplicate resource_def type '{type_name}': first defined in {prev_file}, also in {file_path}"
        )));
    }

    for (param_name, param_spec) in &def.params {
        if !is_env_safe_name(param_name) {
            return Err(Error::Config(format!(
                "resource_def '{type_name}' in {file_path}: param name '{param_name}' is not a valid identifier (must match [a-zA-Z_][a-zA-Z0-9_]*)"
            )));
        }

        if RESERVED_PARAM_NAMES.contains(&param_name.as_str()) {
            return Err(Error::Config(format!(
                "resource_def '{type_name}' in {file_path}: param name '{param_name}' is reserved"
            )));
        }

        if !ALLOWED_PARAM_TYPES.contains(&param_spec.param_type.as_str()) {
            return Err(Error::Config(format!(
                "resource_def '{type_name}' in {file_path}: param '{param_name}' has unknown type '{}'; allowed types are: string, integer, float, boolean",
                param_spec.param_type
            )));
        }

        if let Some(default_val) = &param_spec.default {
            validate_default_value(
                type_name,
                param_name,
                &param_spec.param_type,
                default_val,
                file_path,
            )?;

            if param_spec.param_type == "string"
                && let Some(enum_values) = &param_spec.enum_values
                && let toml::Value::String(s) = default_val
                && !enum_values.contains(s)
            {
                return Err(Error::Config(format!(
                    "resource_def '{type_name}' in {file_path}: param '{param_name}' default '{s}' is not in enum values: {}",
                    enum_values.join(", ")
                )));
            }
        }
    }

    Ok(())
}

fn validate_default_value(
    type_name: &str,
    param_name: &str,
    param_type: &str,
    default_val: &toml::Value,
    file_path: &str,
) -> Result<(), Error> {
    // Only the four scalar kinds are accepted, and only when they match the
    // declared param_type. Array/Table/Datetime fall through to the catch-all.
    let type_matches = matches!(
        (param_type, default_val),
        ("string", toml::Value::String(_))
            | ("integer", toml::Value::Integer(_))
            | ("float", toml::Value::Float(_))
            | ("boolean", toml::Value::Boolean(_))
    );

    if !type_matches {
        let actual_kind = match default_val {
            toml::Value::String(_) => "string",
            toml::Value::Integer(_) => "integer",
            toml::Value::Float(_) => "float",
            toml::Value::Boolean(_) => "boolean",
            toml::Value::Array(_) => "array",
            toml::Value::Table(_) => "table",
            toml::Value::Datetime(_) => "datetime",
        };
        return Err(Error::Config(format!(
            "resource_def '{type_name}' in {file_path}: param '{param_name}' has type '{param_type}' but default is {actual_kind}"
        )));
    }

    Ok(())
}

pub fn load_resource_defs(
    dir: &Path,
    builtin_types: &[&str],
) -> Result<HashMap<String, ResourceDef>, Error> {
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

    let mut result: HashMap<String, ResourceDef> = HashMap::new();
    let mut seen_types: HashMap<String, String> = HashMap::new();

    for entry in entries {
        let path = entry.path();
        let file_path = path.display().to_string();
        let content = std::fs::read_to_string(&path)
            .map_err(|e| Error::Config(format!("failed to read {file_path}: {e}")))?;
        let def_file: DefFile = toml::from_str(&content)
            .map_err(|e| Error::Config(format!("failed to parse {file_path}: {e}")))?;

        for (type_name, def) in def_file.resource_def {
            validate_resource_def(&type_name, &def, builtin_types, &seen_types, &file_path)?;
            seen_types.insert(type_name.clone(), file_path.clone());
            result.insert(type_name, def);
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::{NamedTempFile, TempDir};

    use super::*;

    fn write_toml(dir: &TempDir, name: &str, content: &str) {
        std::fs::write(dir.path().join(name), content).unwrap();
    }

    #[test]
    fn parse_resource_def_with_params_defaults_enum() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"
[resource_def.deploy]
description = "Deploy an application"
check = "test -d /opt/app"
apply = "mkdir -p /opt/app"

[resource_def.deploy.params.path]
type = "string"
required = true

[resource_def.deploy.params.mode]
type = "string"
default = "stable"
enum = ["stable", "canary", "dev"]
"#
        )
        .unwrap();

        let content = std::fs::read_to_string(f.path()).unwrap();
        let def_file: DefFile = toml::from_str(&content).unwrap();
        let deploy = def_file.resource_def.get("deploy").unwrap();

        assert_eq!(deploy.description, "Deploy an application");
        assert_eq!(deploy.check, "test -d /opt/app");
        assert_eq!(deploy.apply, "mkdir -p /opt/app");

        let path_param = deploy.params.get("path").unwrap();
        assert_eq!(path_param.param_type, "string");
        assert!(path_param.required);
        assert!(path_param.default.is_none());

        let mode_param = deploy.params.get("mode").unwrap();
        assert_eq!(mode_param.param_type, "string");
        assert!(!mode_param.required);
        assert_eq!(
            mode_param.default,
            Some(toml::Value::String("stable".to_string()))
        );
        assert_eq!(
            mode_param.enum_values,
            Some(vec![
                "stable".to_string(),
                "canary".to_string(),
                "dev".to_string()
            ])
        );
    }

    #[test]
    fn load_resource_defs_missing_dir_returns_empty() {
        let dir = TempDir::new().unwrap();
        let missing = dir.path().join("does_not_exist");
        let result = load_resource_defs(&missing, &[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn load_resource_defs_duplicate_type_across_files_is_error() {
        let dir = TempDir::new().unwrap();
        write_toml(
            &dir,
            "a.toml",
            r#"
[resource_def.myapp]
check = "test -x /usr/bin/myapp"
apply = "install myapp"
"#,
        );
        write_toml(
            &dir,
            "b.toml",
            r#"
[resource_def.myapp]
check = "test -x /usr/bin/myapp"
apply = "install myapp"
"#,
        );

        let err = load_resource_defs(dir.path(), &[]).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("duplicate"));
        assert!(err.to_string().contains("myapp"));
    }

    #[test]
    fn load_resource_defs_builtin_collision_is_error() {
        let dir = TempDir::new().unwrap();
        write_toml(
            &dir,
            "defs.toml",
            r#"
[resource_def.file]
check = "test -f {{ path }}"
apply = "touch {{ path }}"
"#,
        );

        let err = load_resource_defs(dir.path(), &["file", "cmd"]).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("file"));
        assert!(err.to_string().contains("built-in"));
    }

    #[test]
    fn load_resource_defs_invalid_param_name_is_error() {
        let dir = TempDir::new().unwrap();
        write_toml(
            &dir,
            "defs.toml",
            r#"
[resource_def.myapp]
check = "true"
apply = "install"

[resource_def.myapp.params."my-param"]
type = "string"
"#,
        );

        let err = load_resource_defs(dir.path(), &[]).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("my-param"));
        assert!(err.to_string().contains("identifier"));
    }

    #[test]
    fn load_resource_defs_reserved_param_name_is_error() {
        let dir = TempDir::new().unwrap();
        write_toml(
            &dir,
            "defs.toml",
            r#"
[resource_def.myapp]
check = "true"
apply = "install"

[resource_def.myapp.params.source]
type = "string"
"#,
        );

        let err = load_resource_defs(dir.path(), &[]).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("source"));
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn load_resource_defs_missing_check_apply_is_error() {
        let dir = TempDir::new().unwrap();
        write_toml(
            &dir,
            "defs.toml",
            r#"
[resource_def.myapp]
description = "missing check and apply"
"#,
        );

        let err = load_resource_defs(dir.path(), &[]).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
    }

    #[test]
    fn load_resource_defs_default_type_mismatch_is_error() {
        let dir = TempDir::new().unwrap();
        write_toml(
            &dir,
            "defs.toml",
            r#"
[resource_def.myapp]
check = "true"
apply = "install"

[resource_def.myapp.params.count]
type = "string"
default = 5
"#,
        );

        let err = load_resource_defs(dir.path(), &[]).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("count"));
        assert!(err.to_string().contains("integer"));
    }

    #[test]
    fn load_resource_defs_string_default_outside_enum_is_error() {
        let dir = TempDir::new().unwrap();
        write_toml(
            &dir,
            "defs.toml",
            r#"
[resource_def.myapp]
check = "true"
apply = "install"

[resource_def.myapp.params.mode]
type = "string"
default = "beta"
enum = ["stable", "canary"]
"#,
        );

        let err = load_resource_defs(dir.path(), &[]).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("mode"));
        assert!(err.to_string().contains("enum"));
    }

    #[test]
    fn load_resource_defs_merges_multiple_files() {
        let dir = TempDir::new().unwrap();
        write_toml(
            &dir,
            "a.toml",
            r#"
[resource_def.appone]
check = "test -x /usr/bin/appone"
apply = "install appone"
"#,
        );
        write_toml(
            &dir,
            "b.toml",
            r#"
[resource_def.apptwo]
check = "test -x /usr/bin/apptwo"
apply = "install apptwo"
"#,
        );

        let defs = load_resource_defs(dir.path(), &[]).unwrap();
        assert_eq!(defs.len(), 2);
        assert!(defs.contains_key("appone"));
        assert!(defs.contains_key("apptwo"));
    }

    #[test]
    fn load_resource_defs_array_default_is_error() {
        let dir = TempDir::new().unwrap();
        write_toml(
            &dir,
            "defs.toml",
            r#"
[resource_def.myapp]
check = "true"
apply = "install"

[resource_def.myapp.params.items]
type = "string"
default = ["a", "b"]
"#,
        );

        let err = load_resource_defs(dir.path(), &[]).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("items"));
        assert!(err.to_string().contains("array"));
    }

    #[test]
    fn load_resource_defs_empty_dir_returns_empty() {
        let dir = TempDir::new().unwrap();
        let result = load_resource_defs(dir.path(), &[]).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn load_resource_defs_ignores_non_toml_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("notes.txt"), "not toml").unwrap();
        std::fs::write(dir.path().join("readme.md"), "# readme").unwrap();
        write_toml(
            &dir,
            "defs.toml",
            r#"
[resource_def.myapp]
check = "true"
apply = "install"
"#,
        );

        let defs = load_resource_defs(dir.path(), &[]).unwrap();
        assert_eq!(defs.len(), 1);
        assert!(defs.contains_key("myapp"));
    }

    #[test]
    fn load_resource_defs_invalid_param_type_is_error() {
        let dir = TempDir::new().unwrap();
        write_toml(
            &dir,
            "defs.toml",
            r#"
[resource_def.myapp]
check = "true"
apply = "install"

[resource_def.myapp.params.count]
type = "number"
"#,
        );

        let err = load_resource_defs(dir.path(), &[]).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("count"));
        assert!(err.to_string().contains("number"));
    }

    #[test]
    fn load_resource_defs_reserved_when_param_is_error() {
        let dir = TempDir::new().unwrap();
        write_toml(
            &dir,
            "defs.toml",
            r#"
[resource_def.myapp]
check = "true"
apply = "install"

[resource_def.myapp.params.when]
type = "string"
"#,
        );

        let err = load_resource_defs(dir.path(), &[]).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("when"));
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn load_resource_defs_reserved_vars_param_is_error() {
        let dir = TempDir::new().unwrap();
        write_toml(
            &dir,
            "defs.toml",
            r#"
[resource_def.myapp]
check = "true"
apply = "install"

[resource_def.myapp.params.vars]
type = "string"
"#,
        );

        let err = load_resource_defs(dir.path(), &[]).unwrap_err();
        assert!(matches!(err, Error::Config(_)));
        assert!(err.to_string().contains("vars"));
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn is_env_safe_name_valid_identifiers() {
        assert!(is_env_safe_name("foo"));
        assert!(is_env_safe_name("_bar"));
        assert!(is_env_safe_name("foo_bar_123"));
        assert!(is_env_safe_name("FOO"));
        assert!(is_env_safe_name("_"));
    }

    #[test]
    fn is_env_safe_name_invalid_identifiers() {
        assert!(!is_env_safe_name("my-param"));
        assert!(!is_env_safe_name("a.b"));
        assert!(!is_env_safe_name("123abc"));
        assert!(!is_env_safe_name(""));
        assert!(!is_env_safe_name("foo bar"));
    }
}
