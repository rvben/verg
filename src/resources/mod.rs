pub mod apt_repo;
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
pub mod user;
pub mod when;

use std::process::Command as ProcessCommand;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::Error;

/// Sentinel prefix/suffix for register references preserved through template rendering.
pub const REGISTER_SENTINEL: &str = "__VERG_REG_";
pub const REGISTER_SENTINEL_END: &str = "__VERG_END__";

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
}

impl ResolvedResource {
    pub fn fqn(&self) -> String {
        format!("{}.{}", self.resource_type, self.name)
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
        Err(e) => ResourceResult {
            resource_type: resource.resource_type.clone(),
            name: resource.name.clone(),
            status: ResourceStatus::Failed,
            diff: None,
            from: None,
            to: None,
            error: Some(e.to_string()),
            output: None,
        },
    }
}

pub fn run_cmd(cmd: &str, args: &[&str]) -> Result<std::process::Output, Error> {
    ProcessCommand::new(cmd)
        .args(args)
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
        .map_err(|e| Error::Resource(format!("failed to write stdin: {e}")))?;

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn resolved_resource_fqn() {
        let r = ResolvedResource {
            resource_type: "pkg".into(),
            name: "nginx".into(),
            props: HashMap::new(),
            after: vec![],
            notify: vec![],
            when: None,
            handler: false,
            register: None,
        };
        assert_eq!(r.fqn(), "pkg.nginx");
    }
}
