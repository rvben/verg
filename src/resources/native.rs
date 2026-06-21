use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;

use crate::error::Error;
use crate::provider_def::ProviderDef;
use crate::resources::{
    ResolvedResource, ResourceResult, ResourceStatus, run_cmd_with_stdin, truncate_stderr,
};

/// Convert a single TOML value to a JSON value for use as a provider param.
///
/// String values are passed through literally with no interpretation. Datetime
/// values become plain ISO-8601 strings. All other types map structurally.
fn toml_to_json_literal(v: &toml::Value) -> serde_json::Value {
    match v {
        toml::Value::String(s) => serde_json::Value::String(s.clone()),
        toml::Value::Integer(i) => serde_json::Value::Number((*i).into()),
        toml::Value::Float(f) => serde_json::json!(f),
        toml::Value::Boolean(b) => serde_json::Value::Bool(*b),
        toml::Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(toml_to_json_literal).collect())
        }
        toml::Value::Table(tbl) => {
            let map = tbl
                .iter()
                .map(|(k, v)| (k.clone(), toml_to_json_literal(v)))
                .collect();
            serde_json::Value::Object(map)
        }
        toml::Value::Datetime(dt) => serde_json::Value::String(dt.to_string()),
    }
}

/// Convert a props map to a JSON object, treating every value literally.
fn props_to_json(props: &HashMap<String, toml::Value>) -> serde_json::Value {
    let map = props
        .iter()
        .map(|(k, v)| (k.clone(), toml_to_json_literal(v)))
        .collect();
    serde_json::Value::Object(map)
}

const PROTOCOL_VERSION: u32 = 1;

static SCRIPT_SEQ: AtomicU64 = AtomicU64::new(0);

/// A materialized provider script removed on drop, so every return path of
/// `execute` cleans up the temp file.
struct TempScript {
    path: PathBuf,
}

impl TempScript {
    /// Create the temp file ATOMICALLY with mode 0600 via `create_new` (so there
    /// is no world-readable window and an existing path/symlink is not followed),
    /// register the cleanup guard BEFORE the fallible write, then write the
    /// source through the returned handle.
    fn create(source: &str) -> Result<Self, Error> {
        for _ in 0..64 {
            let seq = SCRIPT_SEQ.fetch_add(1, Ordering::Relaxed);
            let path =
                std::env::temp_dir().join(format!("verg-provider-{}-{seq}", std::process::id()));
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(mut file) => {
                    // Guard now owns the path; if the write fails, Drop removes it.
                    let guard = TempScript { path };
                    file.write_all(source.as_bytes()).map_err(|e| {
                        Error::Resource(format!("failed to write provider script: {e}"))
                    })?;
                    return Ok(guard);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => {
                    return Err(Error::Resource(format!(
                        "failed to create provider script: {e}"
                    )));
                }
            }
        }
        Err(Error::Resource(
            "failed to create a unique provider script temp file".into(),
        ))
    }
}

impl Drop for TempScript {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Debug, Deserialize)]
struct ProviderResponse {
    status: String,
    #[serde(default)]
    diff: Option<String>,
    #[serde(default)]
    output: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

/// Execute a native provider resource over the JSON-over-stdio protocol.
///
/// The provider receives `{protocol_version, action, type, name, params}` on
/// stdin and returns `{status, diff?, output?, error?}` on stdout. `dry_run`
/// sends `action: "plan"` (the provider must not mutate). Protocol violations
/// (non-zero exit, unparseable stdout, unknown status) are reported as a Failed
/// result, never as an `Err`, so one misbehaving provider does not abort the run.
pub fn execute(
    resource: &ResolvedResource,
    def: &ProviderDef,
    dry_run: bool,
) -> Result<ResourceResult, Error> {
    let rtype = &resource.resource_type;
    let name = &resource.name;
    let action = if dry_run { "plan" } else { "apply" };

    let params = props_to_json(&resource.props);
    let request = serde_json::json!({
        "protocol_version": PROTOCOL_VERSION,
        "action": action,
        "type": rtype,
        "name": name,
        "params": params,
    });
    let request_bytes = serde_json::to_vec(&request)
        .map_err(|e| Error::Resource(format!("{rtype} provider: failed to encode request: {e}")))?;

    let script = TempScript::create(&def.source)?;
    let script_path = script.path.to_string_lossy().into_owned();

    let (program, inter_args) = def
        .interpreter
        .split_first()
        .ok_or_else(|| Error::Resource(format!("{rtype} provider: interpreter is empty")))?;
    let mut args: Vec<&str> = inter_args.iter().map(String::as_str).collect();
    args.push(&script_path);

    let output = run_cmd_with_stdin(program, &args, &request_bytes)?;
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "signal".to_string());
        return Ok(ResourceResult::failed(
            rtype,
            name,
            format!(
                "{rtype} provider exited {code}: {}",
                truncate_stderr(&stderr)
            ),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let resp: ProviderResponse = match serde_json::from_str(stdout.trim()) {
        Ok(r) => r,
        Err(e) => {
            return Ok(ResourceResult::failed(
                rtype,
                name,
                format!("{rtype} provider returned invalid JSON response: {e}"),
            ));
        }
    };

    let mut result = match resp.status.as_str() {
        "ok" => ResourceResult::ok(rtype, name),
        "changed" => ResourceResult::changed(
            rtype,
            name,
            resp.diff
                .clone()
                .unwrap_or_else(|| if dry_run { "would change" } else { "changed" }.to_string()),
        ),
        "failed" => ResourceResult::failed(
            rtype,
            name,
            resp.error
                .clone()
                .unwrap_or_else(|| format!("{rtype} provider reported failure")),
        ),
        other => ResourceResult::failed(
            rtype,
            name,
            format!("{rtype} provider returned invalid status '{other}'"),
        ),
    };

    // Pass provider output through for register, but only on a non-failed
    // result (mirrors `cmd`, which captures output on success).
    if result.status != ResourceStatus::Failed && result.output.is_none() {
        result.output = resp.output;
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::provider_def::ProviderDef;
    use crate::resources::{ResolvedResource, ResourceStatus};

    fn resource(props: HashMap<String, toml::Value>) -> ResolvedResource {
        ResolvedResource {
            resource_type: "myprov".into(),
            name: "inst".into(),
            props,
            after: vec![],
            notify: vec![],
            when: None,
            handler: false,
            register: None,
            sensitive: false,
        }
    }

    fn sh_provider(source: &str) -> ProviderDef {
        ProviderDef {
            description: String::new(),
            interpreter: vec!["/bin/sh".into()],
            source: source.into(),
            params: HashMap::new(),
        }
    }

    #[test]
    fn status_ok_maps_to_ok() {
        let def = sh_provider(r#"printf '%s' '{"status":"ok"}'"#);
        let r = execute(&resource(HashMap::new()), &def, false).unwrap();
        assert_eq!(r.status, ResourceStatus::Ok);
    }

    #[test]
    fn status_changed_carries_diff() {
        let def = sh_provider(r#"printf '%s' '{"status":"changed","diff":"created X"}'"#);
        let r = execute(&resource(HashMap::new()), &def, false).unwrap();
        assert_eq!(r.status, ResourceStatus::Changed);
        assert_eq!(r.diff.as_deref(), Some("created X"));
    }

    #[test]
    fn status_failed_carries_error() {
        let def = sh_provider(r#"printf '%s' '{"status":"failed","error":"boom"}'"#);
        let r = execute(&resource(HashMap::new()), &def, false).unwrap();
        assert_eq!(r.status, ResourceStatus::Failed);
        assert!(r.error.unwrap().contains("boom"));
    }

    #[test]
    fn output_is_passed_through_for_register() {
        let def = sh_provider(r#"printf '%s' '{"status":"changed","output":"token123"}'"#);
        let r = execute(&resource(HashMap::new()), &def, false).unwrap();
        assert_eq!(r.output.as_deref(), Some("token123"));
    }

    #[test]
    fn output_is_dropped_on_failed_status() {
        let def =
            sh_provider(r#"printf '%s' '{"status":"failed","error":"boom","output":"leak"}'"#);
        let r = execute(&resource(HashMap::new()), &def, false).unwrap();
        assert_eq!(r.status, ResourceStatus::Failed);
        assert!(
            r.output.is_none(),
            "failed provider output must not be captured"
        );
    }

    #[test]
    fn non_json_stdout_is_failed() {
        let def = sh_provider(r#"printf '%s' 'not json'"#);
        let r = execute(&resource(HashMap::new()), &def, false).unwrap();
        assert_eq!(r.status, ResourceStatus::Failed);
    }

    #[test]
    fn unknown_status_is_failed() {
        let def = sh_provider(r#"printf '%s' '{"status":"weird"}'"#);
        let r = execute(&resource(HashMap::new()), &def, false).unwrap();
        assert_eq!(r.status, ResourceStatus::Failed);
        assert!(r.error.unwrap().contains("weird"));
    }

    #[test]
    fn nonzero_exit_is_failed_and_surfaces_stderr() {
        let def = sh_provider("echo trouble >&2\nexit 1");
        let r = execute(&resource(HashMap::new()), &def, false).unwrap();
        assert_eq!(r.status, ResourceStatus::Failed);
        assert!(r.error.unwrap().contains("trouble"));
    }

    #[test]
    fn plan_action_must_not_mutate_apply_does() {
        let dir = tempfile::tempdir().unwrap();
        let sentinel = dir.path().join("did-apply").to_string_lossy().into_owned();
        // The provider reads the JSON request on stdin; on "apply" it touches the
        // sentinel. We assert plan does not create it and apply does.
        let src = format!(
            "req=$(cat)\ncase \"$req\" in\n*'\"action\":\"apply\"'*) : > '{sentinel}'; printf '%s' '{{\"status\":\"changed\"}}' ;;\n*) printf '%s' '{{\"status\":\"changed\",\"diff\":\"would change\"}}' ;;\nesac\n"
        );
        let def = sh_provider(&src);

        let r = execute(&resource(HashMap::new()), &def, true).unwrap();
        assert_eq!(r.status, ResourceStatus::Changed);
        assert!(
            !std::path::Path::new(&sentinel).exists(),
            "plan must not mutate"
        );

        let r2 = execute(&resource(HashMap::new()), &def, false).unwrap();
        assert_eq!(r2.status, ResourceStatus::Changed);
        assert!(
            std::path::Path::new(&sentinel).exists(),
            "apply must mutate"
        );
    }

    #[test]
    fn request_carries_protocol_action_type_name_and_params() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir
            .path()
            .join("request.json")
            .to_string_lossy()
            .into_owned();
        // Capture the exact request the agent sent.
        let def = sh_provider(&format!(
            "cat > '{out}'\nprintf '%s' '{{\"status\":\"ok\"}}'"
        ));

        let mut props = HashMap::new();
        props.insert(
            "zone".to_string(),
            toml::Value::String("example.com".into()),
        );
        let r = execute(&resource(props), &def, true).unwrap();
        assert_eq!(r.status, ResourceStatus::Ok);

        let req = std::fs::read_to_string(&out).unwrap();
        assert!(req.contains("\"protocol_version\":1"), "got: {req}");
        assert!(req.contains("\"action\":\"plan\""), "got: {req}");
        assert!(req.contains("\"type\":\"myprov\""), "got: {req}");
        assert!(req.contains("\"name\":\"inst\""), "got: {req}");
        assert!(req.contains("example.com"), "params must be present: {req}");
    }

    #[test]
    fn temp_script_is_cleaned_up() {
        // A run leaves no verg-provider-* files behind in the temp dir.
        let before = count_provider_temp_files();
        let def = sh_provider(r#"printf '%s' '{"status":"ok"}'"#);
        let _ = execute(&resource(HashMap::new()), &def, false).unwrap();
        let after = count_provider_temp_files();
        assert_eq!(before, after, "temp script must be removed after execute");
    }

    fn count_provider_temp_files() -> usize {
        let prefix = format!("verg-provider-{}-", std::process::id());
        std::fs::read_dir(std::env::temp_dir())
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
                    .count()
            })
            .unwrap_or(0)
    }

    #[test]
    fn datetime_param_is_a_plain_string_not_a_tagged_object() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir
            .path()
            .join("request.json")
            .to_string_lossy()
            .into_owned();
        let def = sh_provider(&format!(
            "cat > '{out}'\nprintf '%s' '{{\"status\":\"ok\"}}'"
        ));

        let dt = "2024-01-02T03:04:05Z"
            .parse::<toml::value::Datetime>()
            .unwrap();
        let mut props = HashMap::new();
        props.insert("at".to_string(), toml::Value::Datetime(dt));

        let r = execute(&resource(props), &def, true).unwrap();
        assert_eq!(r.status, ResourceStatus::Ok);

        let req = std::fs::read_to_string(&out).unwrap();
        assert!(
            req.contains("2024-01-02"),
            "datetime text must appear in request: {req}"
        );
        assert!(
            !req.contains("$__toml_private_datetime"),
            "tagged TOML datetime object must not appear in request: {req}"
        );
    }

    #[test]
    fn env_string_param_is_passed_through_literally() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir
            .path()
            .join("request.json")
            .to_string_lossy()
            .into_owned();
        let def = sh_provider(&format!(
            "cat > '{out}'\nprintf '%s' '{{\"status\":\"ok\"}}'"
        ));

        let mut props = HashMap::new();
        props.insert("path".to_string(), toml::Value::String("$env.HOME".into()));

        let r = execute(&resource(props), &def, true).unwrap();
        assert_eq!(r.status, ResourceStatus::Ok);

        let req = std::fs::read_to_string(&out).unwrap();
        assert!(
            req.contains("$env.HOME"),
            "literal string must be present unchanged: {req}"
        );
        // The provider must not have expanded it to an actual path.
        assert!(
            !req.contains("/Users/") && !req.contains("/home/"),
            "env var must not be expanded by the param converter: {req}"
        );
    }
}
