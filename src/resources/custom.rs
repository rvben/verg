use std::collections::HashMap;

use crate::error::Error;
use crate::resource_def::ResourceDef;
use crate::resources::{ResolvedResource, ResourceResult, run_cmd_with_env, truncate_stderr};

/// Execute a custom resource defined by a `ResourceDef`.
///
/// Check script exit codes:
/// - `0`: already converged (Ok)
/// - `1`: drift detected; run apply (or report Changed in dry-run)
/// - `>=2`: check itself failed (Failed)
/// - signal: check terminated abnormally (Failed)
///
/// Param values are exposed to both check and apply scripts exclusively as
/// environment variables named `VERG_PARAM_<param_name>`. Values are never
/// interpolated into the command strings, preventing shell injection.
pub fn execute(
    resource: &ResolvedResource,
    def: &ResourceDef,
    dry_run: bool,
) -> Result<ResourceResult, Error> {
    let rtype = &resource.resource_type;
    let name = &resource.name;

    let env = build_env(resource, def);

    // Run the check script.
    let check_output = run_cmd_with_env("sh", &["-c", &def.check], &env)?;
    let check_stderr = String::from_utf8_lossy(&check_output.stderr).into_owned();
    let check_stdout = String::from_utf8_lossy(&check_output.stdout)
        .trim()
        .to_string();

    match check_output.status.code() {
        Some(0) => {
            // Already converged.
            return Ok(ResourceResult::ok(rtype, name));
        }
        Some(1) => {
            // Drift detected; fall through to apply logic below.
        }
        Some(n) => {
            return Ok(ResourceResult::failed(
                rtype,
                name,
                format!(
                    "{rtype} check failed (exit {n}): {}",
                    truncate_stderr(&check_stderr)
                ),
            ));
        }
        None => {
            return Ok(ResourceResult::failed(
                rtype,
                name,
                format!(
                    "{rtype} check terminated by signal: {}",
                    truncate_stderr(&check_stderr)
                ),
            ));
        }
    }

    // Drift detected (check exited 1).
    let diff = if check_stdout.is_empty() {
        None
    } else {
        Some(check_stdout.clone())
    };

    if dry_run {
        let diff_str = diff.unwrap_or_else(|| "would change".to_string());
        return Ok(ResourceResult::changed(rtype, name, diff_str));
    }

    // Run the apply script.
    let apply_output = run_cmd_with_env("sh", &["-c", &def.apply], &env)?;

    if apply_output.status.success() {
        let diff_str = diff.unwrap_or_else(|| "changed".to_string());
        Ok(ResourceResult::changed(rtype, name, diff_str))
    } else {
        let apply_stderr = String::from_utf8_lossy(&apply_output.stderr).into_owned();
        Ok(ResourceResult::failed(
            rtype,
            name,
            format!("{rtype} apply failed: {}", truncate_stderr(&apply_stderr)),
        ))
    }
}

/// Build the environment map for check/apply scripts.
///
/// For each param declared in the def:
///   - Use the instance prop if present (already interpolated at bundle build time).
///   - Fall back to the param's default if declared.
///   - If neither exists, skip the variable entirely.
///
/// Values are keyed as `VERG_PARAM_<name>`.
fn build_env(resource: &ResolvedResource, def: &ResourceDef) -> HashMap<String, String> {
    let mut env = HashMap::new();
    for (param_name, param_spec) in &def.params {
        let value: Option<String> = if let Some(prop_val) = resource.props.get(param_name) {
            // Instance prop: convert toml::Value to String.
            toml_value_to_string(prop_val)
        } else if let Some(default_val) = &param_spec.default {
            // Default: literal value from the def.
            toml_value_to_string(default_val)
        } else {
            None
        };

        if let Some(v) = value {
            env.insert(format!("VERG_PARAM_{param_name}"), v);
        }
    }
    env
}

/// Convert a scalar `toml::Value` to its String representation.
/// Returns `None` for non-scalar types (arrays, tables, datetimes).
fn toml_value_to_string(val: &toml::Value) -> Option<String> {
    match val {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Integer(i) => Some(i.to_string()),
        toml::Value::Float(f) => Some(f.to_string()),
        toml::Value::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tempfile::TempDir;

    use super::*;
    use crate::resource_def::{ParamSpec, ResourceDef};
    use crate::resources::{ResolvedResource, ResourceStatus};

    fn make_resource(
        rtype: &str,
        name: &str,
        props: HashMap<String, toml::Value>,
    ) -> ResolvedResource {
        ResolvedResource {
            resource_type: rtype.to_string(),
            name: name.to_string(),
            props,
            after: vec![],
            notify: vec![],
            when: None,
            handler: false,
            register: None,
            sensitive: false,
        }
    }

    fn make_def(check: &str, apply: &str) -> ResourceDef {
        ResourceDef {
            description: "test".into(),
            params: HashMap::new(),
            check: check.to_string(),
            apply: apply.to_string(),
        }
    }

    fn make_def_with_params(
        check: &str,
        apply: &str,
        params: HashMap<String, ParamSpec>,
    ) -> ResourceDef {
        ResourceDef {
            description: "test".into(),
            params,
            check: check.to_string(),
            apply: apply.to_string(),
        }
    }

    fn make_param(required: bool, default: Option<toml::Value>) -> ParamSpec {
        ParamSpec {
            param_type: "string".to_string(),
            required,
            default,
            enum_values: None,
        }
    }

    // --- basic check/apply lifecycle ---

    #[test]
    fn check_pass_is_ok() {
        let resource = make_resource("mytype", "thing", HashMap::new());
        let def = make_def("true", "true");
        let result = execute(&resource, &def, false).unwrap();
        assert_eq!(result.status, ResourceStatus::Ok);
        assert_eq!(result.resource_type, "mytype");
        assert_eq!(result.name, "thing");
    }

    #[test]
    fn drift_then_apply_creates_file_and_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sentinel").to_string_lossy().to_string();

        let mut params = HashMap::new();
        params.insert("path".to_string(), make_param(true, None));

        let def = make_def_with_params(
            r#"test -f "$VERG_PARAM_path""#,
            r#"touch "$VERG_PARAM_path""#,
            params,
        );

        let mut props = HashMap::new();
        props.insert("path".to_string(), toml::Value::String(path.clone()));
        let resource = make_resource("mytype", "sentinel", props.clone());

        // First run: file does not exist -> drift -> apply -> Changed, file now exists.
        let result = execute(&resource, &def, false).unwrap();
        assert_eq!(
            result.status,
            ResourceStatus::Changed,
            "expected Changed on first run"
        );
        assert!(
            std::path::Path::new(&path).exists(),
            "apply must have created the file"
        );

        // Second run: file exists -> Ok (idempotent).
        let result2 = execute(&resource, &def, false).unwrap();
        assert_eq!(
            result2.status,
            ResourceStatus::Ok,
            "expected Ok on second run (idempotent)"
        );
    }

    #[test]
    fn dry_run_does_not_create_file() {
        let dir = TempDir::new().unwrap();
        let path = dir
            .path()
            .join("dry-sentinel")
            .to_string_lossy()
            .to_string();

        let mut params = HashMap::new();
        params.insert("path".to_string(), make_param(true, None));

        let def = make_def_with_params(
            r#"test -f "$VERG_PARAM_path""#,
            r#"touch "$VERG_PARAM_path""#,
            params,
        );

        let mut props = HashMap::new();
        props.insert("path".to_string(), toml::Value::String(path.clone()));
        let resource = make_resource("mytype", "dry-sentinel", props);

        // dry_run=true: drift is detected but apply must NOT run.
        let result = execute(&resource, &def, true).unwrap();
        assert_eq!(
            result.status,
            ResourceStatus::Changed,
            "expected Changed in dry-run"
        );
        assert!(
            !std::path::Path::new(&path).exists(),
            "dry-run must NOT create the file"
        );
    }

    // --- error paths ---

    #[test]
    fn check_exit_2_is_failed_with_exit_code_in_message() {
        let resource = make_resource("mytype", "badcheck", HashMap::new());
        let def = make_def("exit 2", "true");
        let result = execute(&resource, &def, false).unwrap();
        assert_eq!(result.status, ResourceStatus::Failed);
        let err = result.error.unwrap();
        assert!(
            err.contains("exit 2"),
            "error must contain 'exit 2', got: {err}"
        );
    }

    #[test]
    fn apply_failure_is_failed() {
        let resource = make_resource("mytype", "badapply", HashMap::new());
        // check exits 1 (drift), apply exits 1 (failure)
        let def = make_def("exit 1", "exit 1");
        let result = execute(&resource, &def, false).unwrap();
        assert_eq!(result.status, ResourceStatus::Failed);
        let err = result.error.unwrap();
        assert!(
            err.contains("apply failed"),
            "error must mention apply failed, got: {err}"
        );
    }

    // --- injection safety ---

    #[test]
    fn param_value_is_not_shell_executed() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("inject-out").to_string_lossy().to_string();

        // The line param contains a shell command substitution. If the value were
        // interpolated into the script string, it would be executed. Passing it as
        // an env var means it is treated as literal text.
        let evil_value = "$(echo pwned)";

        let mut params = HashMap::new();
        params.insert("line".to_string(), make_param(true, None));
        params.insert("path".to_string(), make_param(true, None));

        let def = make_def_with_params(
            "exit 1", // always drift so apply runs
            r#"printf '%s' "$VERG_PARAM_line" > "$VERG_PARAM_path""#,
            params,
        );

        let mut props = HashMap::new();
        props.insert(
            "line".to_string(),
            toml::Value::String(evil_value.to_string()),
        );
        props.insert("path".to_string(), toml::Value::String(path.clone()));
        let resource = make_resource("mytype", "inject", props);

        let result = execute(&resource, &def, false).unwrap();
        assert_eq!(result.status, ResourceStatus::Changed);

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            written, evil_value,
            "file must contain the LITERAL string, not the result of shell evaluation"
        );
    }

    // --- defaults ---

    #[test]
    fn param_default_is_used_when_no_instance_prop() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("default-out").to_string_lossy().to_string();

        let mut params = HashMap::new();
        params.insert(
            "greeting".to_string(),
            ParamSpec {
                param_type: "string".to_string(),
                required: false,
                default: Some(toml::Value::String("hello-default".to_string())),
                enum_values: None,
            },
        );
        params.insert("path".to_string(), make_param(true, None));

        // check always drifts; apply writes the greeting param to a file.
        let def = make_def_with_params(
            "exit 1",
            r#"printf '%s' "$VERG_PARAM_greeting" > "$VERG_PARAM_path""#,
            params,
        );

        // Only provide "path", NOT "greeting" -> should fall back to default.
        let mut props = HashMap::new();
        props.insert("path".to_string(), toml::Value::String(path.clone()));
        let resource = make_resource("mytype", "defaults", props);

        let result = execute(&resource, &def, false).unwrap();
        assert_eq!(result.status, ResourceStatus::Changed);

        let written = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            written, "hello-default",
            "default value must reach the script when no instance prop is set"
        );
    }

    // --- NUL byte rejection via run_cmd_with_env ---

    #[test]
    fn nul_byte_in_param_value_is_rejected() {
        let mut params = HashMap::new();
        params.insert("p".to_string(), make_param(true, None));

        let def = make_def_with_params("exit 1", "true", params);

        let mut props = HashMap::new();
        // A NUL byte embedded in a param value must be rejected before spawning.
        props.insert("p".to_string(), toml::Value::String("ab\0cd".to_string()));
        let resource = make_resource("mytype", "nul", props);

        let result = execute(&resource, &def, false);
        // run_cmd_with_env should return Err before spawning.
        assert!(result.is_err(), "NUL byte in env value must return Err");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.to_lowercase().contains("nul") || msg.to_lowercase().contains("null"),
            "error message must mention NUL, got: {msg}"
        );
    }
}
