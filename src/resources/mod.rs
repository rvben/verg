pub mod apt_repo;
pub mod atomic;
pub mod cmd;
pub mod cron;
pub mod dag;
pub mod directory;
pub mod docker_compose;
pub mod download;
pub mod file;
pub mod pkg;
pub mod service;
pub mod sysctl;
pub mod tempdir;
pub mod user;
pub mod when;

use std::io::Read;
use std::process::Command as ProcessCommand;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::Error;

/// Minimal secure PATH for root command resolution (independent of the inherited
/// environment). Includes /usr/local/bin for docker/compose.
pub const SECURE_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Read up to `max` bytes as UTF-8; error if the source exceeds `max` (so a
/// runaway bundle cannot OOM the agent).
pub fn read_bounded<R: std::io::Read>(reader: R, max: usize) -> Result<String, Error> {
    let mut buf = Vec::new();
    // Read one extra byte so we can detect an over-limit source.
    let read = reader.take((max as u64) + 1).read_to_end(&mut buf)?;
    if read > max {
        return Err(Error::Config(format!(
            "input too large: exceeds {max} bytes"
        )));
    }
    String::from_utf8(buf).map_err(|e| Error::Parse(format!("input is not valid UTF-8: {e}")))
}

/// Sentinel prefix/suffix for register references preserved through template rendering.
pub const REGISTER_SENTINEL: &str = "__VERG_REG_";
pub const REGISTER_SENTINEL_END: &str = "__VERG_END__";

/// Parse an octal mode string (e.g. "0644") into its numeric value.
pub fn parse_octal_mode(mode: &str) -> Result<u32, Error> {
    u32::from_str_radix(mode, 8).map_err(|_| Error::Resource(format!("invalid mode: {mode}")))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ResourceStatus {
    Ok,
    Changed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceResult {
    #[serde(rename = "type")]
    pub resource_type: String,
    pub name: String,
    pub status: ResourceStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunSummary {
    pub host: String,
    pub resources: Vec<ResourceResult>,
    pub summary: SummaryCount,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummaryCount {
    pub changed: usize,
    pub ok: usize,
    pub failed: usize,
    pub skipped: usize,
}

impl RunSummary {
    pub fn from_results(host: &str, resources: Vec<ResourceResult>) -> Self {
        let summary = SummaryCount {
            changed: resources
                .iter()
                .filter(|r| r.status == ResourceStatus::Changed)
                .count(),
            ok: resources
                .iter()
                .filter(|r| r.status == ResourceStatus::Ok)
                .count(),
            failed: resources
                .iter()
                .filter(|r| r.status == ResourceStatus::Failed)
                .count(),
            skipped: resources
                .iter()
                .filter(|r| r.status == ResourceStatus::Skipped)
                .count(),
        };
        RunSummary {
            host: host.to_string(),
            resources,
            summary,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedResource {
    pub resource_type: String,
    pub name: String,
    pub props: HashMap<String, toml::Value>,
    #[serde(default)]
    pub after: Vec<String>,
    #[serde(default)]
    pub notify: Vec<String>,
    #[serde(default)]
    pub when: Option<String>,
    #[serde(default)]
    pub handler: bool,
    #[serde(default)]
    pub register: Option<String>,
    #[serde(default)]
    pub sensitive: bool,
}

impl ResolvedResource {
    pub fn fqn(&self) -> String {
        format!("{}.{}", self.resource_type, self.name)
    }

    /// An optional string property.
    pub fn prop_str(&self, key: &str) -> Option<&str> {
        self.props.get(key).and_then(|v| v.as_str())
    }

    /// A string property, or `default` when absent or non-string.
    pub fn prop_str_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.prop_str(key).unwrap_or(default)
    }

    /// A required string property; errors if absent or non-string.
    pub fn prop_str_required(&self, key: &str) -> Result<&str, Error> {
        self.prop_str(key).ok_or_else(|| {
            Error::Resource(format!(
                "{} resource requires '{}'",
                self.resource_type, key
            ))
        })
    }

    /// A boolean property, or `default` when absent or non-bool.
    pub fn prop_bool_or(&self, key: &str, default: bool) -> bool {
        self.props
            .get(key)
            .and_then(|v| v.as_bool())
            .unwrap_or(default)
    }
}

impl ResourceResult {
    /// A resource already in the desired state (no change).
    pub fn ok(resource_type: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            resource_type: resource_type.into(),
            name: name.into(),
            status: ResourceStatus::Ok,
            diff: None,
            from: None,
            to: None,
            error: None,
            output: None,
        }
    }

    /// A resource that was changed, with a human-readable diff summary.
    pub fn changed(
        resource_type: impl Into<String>,
        name: impl Into<String>,
        diff: impl Into<String>,
    ) -> Self {
        Self {
            resource_type: resource_type.into(),
            name: name.into(),
            status: ResourceStatus::Changed,
            diff: Some(diff.into()),
            from: None,
            to: None,
            error: None,
            output: None,
        }
    }

    /// Build an Ok result when `changes` is empty, otherwise a Changed result
    /// whose diff is the comma-joined change list.
    pub fn from_changes(
        resource_type: impl Into<String>,
        name: impl Into<String>,
        changes: &[String],
    ) -> Self {
        if changes.is_empty() {
            Self::ok(resource_type, name)
        } else {
            Self::changed(resource_type, name, changes.join(", "))
        }
    }

    /// A resource that failed, carrying the error message.
    pub fn failed(
        resource_type: impl Into<String>,
        name: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            resource_type: resource_type.into(),
            name: name.into(),
            status: ResourceStatus::Failed,
            diff: None,
            from: None,
            to: None,
            error: Some(error.into()),
            output: None,
        }
    }
}

pub fn execute_resource(
    resource: &ResolvedResource,
    dry_run: bool,
    notified: bool,
) -> ResourceResult {
    let result = match resource.resource_type.as_str() {
        "apt_repo" => apt_repo::execute(resource, dry_run),
        "directory" => directory::execute(resource, dry_run),
        "docker_compose" => docker_compose::execute(resource, dry_run),
        "download" => download::execute(resource, dry_run),
        "pkg" => pkg::execute(resource, dry_run),
        "file" => file::execute(resource, dry_run),
        "service" => service::execute(resource, dry_run),
        "sysctl" => sysctl::execute(resource, dry_run),
        "cmd" => cmd::execute(resource, dry_run, notified),
        "cron" => cron::execute(resource, dry_run),
        "user" => user::execute(resource, dry_run),
        other => Err(Error::Resource(format!("unknown resource type: {other}"))),
    };

    match result {
        Ok(r) => r,
        Err(e) => ResourceResult::failed(
            resource.resource_type.clone(),
            resource.name.clone(),
            e.to_string(),
        ),
    }
}

/// Blank a result's payload fields when the resource is marked sensitive, so a
/// secret never reaches stdout/JSON. The status and error are kept.
pub fn redact_result(mut result: ResourceResult, sensitive: bool) -> ResourceResult {
    if sensitive {
        result.from = None;
        result.to = None;
        result.output = None;
        if result.diff.is_some() {
            result.diff = Some("[redacted]".into());
        }
    }
    result
}

/// Run a command and turn a non-zero exit into `Error::Resource("{ctx} failed: {stderr}")`.
pub fn run_checked(cmd: &str, args: &[&str], ctx: &str) -> Result<(), Error> {
    let output = run_cmd(cmd, args)?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Resource(format!("{ctx} failed: {stderr}")));
    }
    Ok(())
}

pub fn run_cmd(cmd: &str, args: &[&str]) -> Result<std::process::Output, Error> {
    ProcessCommand::new(cmd)
        .args(args)
        .env("PATH", SECURE_PATH)
        .output()
        .map_err(|e| Error::Resource(format!("failed to run {cmd}: {e}")))
}

/// Run a command, piping `stdin_data` to its stdin.
///
/// Spawns a thread to write stdin concurrently with collecting stdout/stderr
/// to prevent deadlock when the process's output buffers fill before all
/// stdin bytes are consumed.
///
/// The caller is responsible for not including stdin content in any user-visible
/// output — treat it as sensitive.
pub fn run_cmd_with_stdin(
    cmd: &str,
    args: &[&str],
    stdin_data: &[u8],
) -> Result<std::process::Output, Error> {
    use std::io::Write;
    use std::process::Stdio;

    let mut child = ProcessCommand::new(cmd)
        .args(args)
        .env("PATH", SECURE_PATH)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Resource(format!("failed to spawn {cmd}: {e}")))?;

    let data = stdin_data.to_vec();
    let mut stdin_pipe = child.stdin.take().expect("stdin is piped");
    let write_thread = std::thread::spawn(move || stdin_pipe.write_all(&data));

    let output = child
        .wait_with_output()
        .map_err(|e| Error::Resource(format!("failed to wait for {cmd}: {e}")))?;

    write_thread
        .join()
        .map_err(|_| Error::Resource("stdin write thread panicked".into()))?
        .or_else(|e| {
            // Broken pipe means the child exited before consuming all stdin,
            // which is valid (e.g. the command ignores its stdin). The exit
            // status collected above is the authoritative result.
            if e.kind() == std::io::ErrorKind::BrokenPipe {
                Ok(())
            } else {
                Err(Error::Resource(format!("failed to write stdin: {e}")))
            }
        })?;

    Ok(output)
}

#[cfg(test)]
pub(crate) fn test_resource(
    resource_type: &str,
    name: &str,
    props: std::collections::HashMap<String, toml::Value>,
) -> ResolvedResource {
    ResolvedResource {
        resource_type: resource_type.to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prop_accessors_read_typed_values() {
        let mut props = HashMap::new();
        props.insert("name".to_string(), toml::Value::String("nginx".into()));
        props.insert("enabled".to_string(), toml::Value::Boolean(true));
        let r = test_resource("service", "nginx", props);
        assert_eq!(r.prop_str("name"), Some("nginx"));
        assert_eq!(r.prop_str("missing"), None);
        assert_eq!(r.prop_str_or("missing", "present"), "present");
        assert_eq!(r.prop_str_or("name", "x"), "nginx");
        assert!(r.prop_bool_or("enabled", false));
        assert!(!r.prop_bool_or("missing", false));
        assert_eq!(r.prop_str_required("name").unwrap(), "nginx");
        let err = r.prop_str_required("missing").unwrap_err();
        assert!(
            err.to_string()
                .contains("service resource requires 'missing'"),
            "got: {err}"
        );
    }

    #[test]
    fn from_changes_empty_is_ok_no_diff() {
        let r = ResourceResult::from_changes("file", "/x", &[]);
        assert_eq!(r.status, ResourceStatus::Ok);
        assert!(r.diff.is_none());
    }

    #[test]
    fn from_changes_nonempty_is_changed_joined() {
        let changes = vec!["mode 0644".to_string(), "owner root".to_string()];
        let r = ResourceResult::from_changes("file", "/x", &changes);
        assert_eq!(r.status, ResourceStatus::Changed);
        assert_eq!(r.diff.as_deref(), Some("mode 0644, owner root"));
    }

    #[test]
    fn resource_result_ok_constructor() {
        let r = ResourceResult::ok("pkg", "nginx");
        assert_eq!(r.resource_type, "pkg");
        assert_eq!(r.name, "nginx");
        assert_eq!(r.status, ResourceStatus::Ok);
        assert!(
            r.diff.is_none()
                && r.from.is_none()
                && r.to.is_none()
                && r.error.is_none()
                && r.output.is_none()
        );
    }

    #[test]
    fn resource_result_changed_constructor() {
        let r = ResourceResult::changed("file", "/etc/hosts", "mode 0644");
        assert_eq!(r.status, ResourceStatus::Changed);
        assert_eq!(r.diff.as_deref(), Some("mode 0644"));
        assert!(r.from.is_none() && r.to.is_none() && r.error.is_none() && r.output.is_none());
    }

    #[test]
    fn resource_result_failed_constructor() {
        let r = ResourceResult::failed("cmd", "deploy", "boom");
        assert_eq!(r.resource_type, "cmd");
        assert_eq!(r.name, "deploy");
        assert_eq!(r.status, ResourceStatus::Failed);
        assert_eq!(r.error.as_deref(), Some("boom"));
        assert!(r.diff.is_none() && r.from.is_none() && r.to.is_none() && r.output.is_none());
    }

    #[test]
    fn redact_result_blanks_payloads_when_sensitive() {
        let r = ResourceResult {
            resource_type: "cmd".into(),
            name: "x".into(),
            status: ResourceStatus::Changed,
            diff: Some("secret diff".into()),
            from: Some("old secret".into()),
            to: Some("new secret".into()),
            error: None,
            output: Some("captured secret".into()),
        };
        let red = redact_result(r, true);
        assert_eq!(red.status, ResourceStatus::Changed);
        assert!(red.from.is_none());
        assert!(red.to.is_none());
        assert!(red.output.is_none());
        assert_eq!(red.diff.as_deref(), Some("[redacted]"));
    }

    #[test]
    fn redact_result_noop_when_not_sensitive() {
        let r = ResourceResult {
            resource_type: "cmd".into(),
            name: "x".into(),
            status: ResourceStatus::Changed,
            diff: Some("d".into()),
            from: None,
            to: None,
            error: None,
            output: Some("o".into()),
        };
        let red = redact_result(r, false);
        assert_eq!(red.diff.as_deref(), Some("d"));
        assert_eq!(red.output.as_deref(), Some("o"));
    }

    #[test]
    fn run_summary_counts() {
        let results = vec![
            ResourceResult {
                resource_type: "pkg".into(),
                name: "nginx".into(),
                status: ResourceStatus::Ok,
                diff: None,
                from: None,
                to: None,
                error: None,
                output: None,
            },
            ResourceResult {
                resource_type: "file".into(),
                name: "conf".into(),
                status: ResourceStatus::Changed,
                diff: Some("...".into()),
                from: None,
                to: None,
                error: None,
                output: None,
            },
            ResourceResult {
                resource_type: "service".into(),
                name: "nginx".into(),
                status: ResourceStatus::Failed,
                diff: None,
                from: None,
                to: None,
                error: Some("not found".into()),
                output: None,
            },
        ];
        let summary = RunSummary::from_results("web1", results);
        assert_eq!(summary.summary.ok, 1);
        assert_eq!(summary.summary.changed, 1);
        assert_eq!(summary.summary.failed, 1);
        assert_eq!(summary.summary.skipped, 0);
    }

    #[test]
    fn parse_octal_mode_valid() {
        assert_eq!(parse_octal_mode("0644").unwrap(), 0o644);
        assert_eq!(parse_octal_mode("755").unwrap(), 0o755);
    }

    #[test]
    fn parse_octal_mode_invalid() {
        let err = parse_octal_mode("xyz").unwrap_err();
        assert!(err.to_string().contains("invalid mode: xyz"), "got: {err}");
    }

    #[test]
    fn run_checked_succeeds_for_true() {
        // `true` exits 0 on every supported platform.
        assert!(run_checked("true", &[], "noop").is_ok());
    }

    #[test]
    fn run_checked_errors_with_ctx_on_failure() {
        // `false` exits non-zero; the ctx must appear in the message.
        let err = run_checked("false", &[], "myctx").unwrap_err();
        assert!(err.to_string().contains("myctx failed"), "got: {err}");
    }

    #[test]
    fn read_bounded_accepts_within_limit() {
        let data = b"hello world";
        let out = read_bounded(&data[..], 1024).unwrap();
        assert_eq!(out, "hello world");
    }

    #[test]
    fn read_bounded_rejects_oversized() {
        let data = vec![b'x'; 100];
        let err = read_bounded(&data[..], 16).unwrap_err();
        assert!(
            err.to_string().contains("too large") || err.to_string().contains("exceeds"),
            "got: {err}"
        );
    }

    #[test]
    fn resolved_resource_fqn() {
        let r = test_resource("pkg", "nginx", HashMap::new());
        assert_eq!(r.fqn(), "pkg.nginx");
    }
}
