pub mod cmd;
pub mod dag;
pub mod file;
pub mod pkg;
pub mod service;
pub mod user;

use std::process::Command as ProcessCommand;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::error::Error;

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
}

impl ResolvedResource {
    pub fn fqn(&self) -> String {
        format!("{}.{}", self.resource_type, self.name)
    }
}

pub fn execute_resource(resource: &ResolvedResource, dry_run: bool) -> ResourceResult {
    let result = match resource.resource_type.as_str() {
        "pkg" => pkg::execute(resource, dry_run),
        "file" => file::execute(resource, dry_run),
        "service" => service::execute(resource, dry_run),
        "cmd" => cmd::execute(resource, dry_run),
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
        },
    }
}

pub fn run_cmd(cmd: &str, args: &[&str]) -> Result<std::process::Output, Error> {
    ProcessCommand::new(cmd)
        .args(args)
        .output()
        .map_err(|e| Error::Resource(format!("failed to run {cmd}: {e}")))
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
            },
            ResourceResult {
                resource_type: "file".into(),
                name: "conf".into(),
                status: ResourceStatus::Changed,
                diff: Some("...".into()),
                from: None,
                to: None,
                error: None,
            },
            ResourceResult {
                resource_type: "service".into(),
                name: "nginx".into(),
                status: ResourceStatus::Failed,
                diff: None,
                from: None,
                to: None,
                error: Some("not found".into()),
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
        };
        assert_eq!(r.fqn(), "pkg.nginx");
    }
}
