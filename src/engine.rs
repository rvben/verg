use std::path::Path;
use std::sync::Arc;

use tokio::task::JoinSet;

use crate::bundle::Bundle;
use crate::error::Error;
use crate::inventory::{Inventory, selector};
use crate::resources::{ResourceResult, ResourceStatus, RunSummary};
use crate::state;
use crate::transport::ssh::SshTransport;

pub struct Engine {
    pub transport: SshTransport,
    pub parallel: usize,
    pub policy: crate::config::ConfigPolicy,
}

#[derive(Debug)]
pub struct EngineResult {
    pub summaries: Vec<RunSummary>,
}

impl EngineResult {
    pub fn has_failures(&self) -> bool {
        self.summaries.iter().any(|s| s.summary.failed > 0)
    }

    pub fn has_changes(&self) -> bool {
        self.summaries.iter().any(|s| s.summary.changed > 0)
    }

    /// Compute the process exit code based on the run outcome.
    /// Failures take priority over changes.
    pub fn exit_code(&self) -> i32 {
        use crate::error::exit_codes;
        if self.has_failures() {
            if self.has_changes() || self.summaries.iter().any(|s| s.summary.ok > 0) {
                exit_codes::PARTIAL_FAILURE
            } else {
                exit_codes::TOTAL_FAILURE
            }
        } else if self.has_changes() {
            exit_codes::SUCCESS
        } else {
            exit_codes::NOTHING_CHANGED
        }
    }
}

impl Engine {
    pub async fn run(
        &self,
        base_dir: &Path,
        target_selector: &str,
        dry_run: bool,
    ) -> Result<EngineResult, Error> {
        let inventory = Inventory::load(base_dir)?;

        // Validate config on the control host before anything host-specific, so
        // typos fail locally and loudly even if the selector matches no hosts.
        let state_dir = base_dir.join("state");
        let state_files = state::load_state_dir(&state_dir)?;
        crate::config::validate_state_files(&state_files, self.policy)?;
        if state_dir.is_dir() {
            let mut entries: Vec<_> = std::fs::read_dir(&state_dir)
                .map_err(|e| Error::Config(format!("failed to read {}: {e}", state_dir.display())))?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
                .map(|e| e.path())
                .collect();
            entries.sort();
            for path in entries {
                let raw = std::fs::read_to_string(&path).map_err(|e| {
                    Error::Config(format!("failed to read {}: {e}", path.display()))
                })?;
                let source = path.file_name().unwrap().to_string_lossy().to_string();
                crate::config::validate_state_file_toml(&raw, &source, self.policy)?;
            }
        }

        let selector = selector::parse_selector(target_selector)?;
        let hosts = inventory.filter(&selector)?;

        // A non-empty selector that matches nothing is an error. The "all"
        // selector on an empty inventory is valid (nothing to do).
        if hosts.is_empty() && !matches!(selector, crate::inventory::selector::Selector::All) {
            return Err(Error::TargetNotFound(target_selector.into()));
        }

        if hosts.is_empty() {
            return Ok(EngineResult { summaries: vec![] });
        }

        let inventory_ctx = Arc::new(inventory.to_template_context());

        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.parallel));
        let mut join_set = JoinSet::new();

        for host in hosts {
            let host = host.clone();
            let state_files = state_files.clone();
            let inventory_ctx = Arc::clone(&inventory_ctx);
            let mut transport = SshTransport::new(
                self.transport.agent_dir.clone(),
                self.transport.version.clone(),
            );
            transport.ssh_config = self.transport.ssh_config.clone();
            let sem = semaphore.clone();

            let base_dir = base_dir.to_path_buf();
            join_set.spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let host_name = host.name.clone();
                let result = async {
                    // Gather facts from target and inject into host vars
                    let facts = transport
                        .gather_facts(&host.user, &host.address, host.port)
                        .await?;

                    let mut host = host;
                    // Inject facts as variables (fact.arch, fact.hostname, etc.)
                    for (k, v) in &facts {
                        host.vars
                            .entry(k.clone())
                            .or_insert_with(|| toml::Value::String(v.clone()));
                    }
                    // Inject group membership as variables (group.docker = "true")
                    for group in &host.groups {
                        host.vars
                            .entry(format!("group.{group}"))
                            .or_insert_with(|| toml::Value::String("true".into()));
                    }

                    let bundle = Bundle::build(&host, &state_files, &base_dir, &inventory_ctx)?;
                    let result = transport
                        .execute(&host.user, &host.address, host.port, &bundle, dry_run)
                        .await?;
                    Ok::<RunSummary, Error>(result.summary)
                }
                .await;

                match result {
                    Ok(summary) => summary,
                    Err(e) => RunSummary::from_results(
                        &host_name,
                        vec![ResourceResult {
                            resource_type: "connection".into(),
                            name: host_name.clone(),
                            status: ResourceStatus::Failed,
                            diff: None,
                            from: None,
                            to: None,
                            output: None,
                            error: Some(e.to_string()),
                        }],
                    ),
                }
            });
        }

        let mut summaries = Vec::new();
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(summary) => summaries.push(summary),
                Err(e) => {
                    summaries.push(RunSummary::from_results(
                        "unknown",
                        vec![ResourceResult {
                            resource_type: "connection".into(),
                            name: "task".into(),
                            status: ResourceStatus::Failed,
                            diff: None,
                            from: None,
                            to: None,
                            output: None,
                            error: Some(format!("task join error: {e}")),
                        }],
                    ));
                }
            }
        }

        Ok(EngineResult { summaries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn run_rejects_typoed_state_key_before_ssh() {
        let dir = tempfile::TempDir::new().unwrap();
        // RFC 5737 TEST-NET-1 address; never actually contacted because
        // validation fails first.
        std::fs::write(
            dir.path().join("hosts.toml"),
            "[hosts.web1]\naddress = \"192.0.2.10\"\n",
        )
        .unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(
            state.join("web.toml"),
            "targetss = [\"web\"]\n[resource.pkg.nginx]\nname = \"nginx\"\n",
        )
        .unwrap();

        let engine = Engine {
            transport: SshTransport::new(std::path::PathBuf::from("/tmp"), "0.0.0".into()),
            parallel: 1,
            policy: crate::config::ConfigPolicy::strict(),
        };
        let err = engine.run(dir.path(), "all", true).await.unwrap_err();
        assert_eq!(
            err.exit_code(),
            crate::error::exit_codes::INVALID_CONFIG,
            "typoed top-level key must fail as invalid_config, got: {err}"
        );
    }
}
