use std::path::Path;

use tokio::task::JoinSet;

use crate::bundle::Bundle;
use crate::error::Error;
use crate::inventory::Inventory;
use crate::inventory::selector;
use crate::resources::RunSummary;
use crate::state;
use crate::transport::ssh::SshTransport;

pub struct Engine {
    pub transport: SshTransport,
    pub parallel: usize,
}

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
}

impl Engine {
    pub async fn run(
        &self,
        base_dir: &Path,
        target_selector: &str,
        dry_run: bool,
    ) -> Result<EngineResult, Error> {
        let inventory = Inventory::load(base_dir)?;
        let selector = selector::parse_selector(target_selector)?;
        let hosts = inventory.filter(&selector)?;

        if hosts.is_empty() {
            return Err(Error::TargetNotFound(target_selector.into()));
        }

        let state_dir = base_dir.join("state");
        let state_files = state::load_state_dir(&state_dir)?;

        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(self.parallel));
        let mut join_set = JoinSet::new();

        for host in hosts {
            let host = host.clone();
            let state_files = state_files.clone();
            let transport = SshTransport::new(
                self.transport.agent_binary.clone(),
                self.transport.version.clone(),
            );
            let sem = semaphore.clone();

            join_set.spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let bundle = Bundle::build(&host, &state_files)?;
                let result = transport
                    .execute(&host.user, &host.address, &bundle, dry_run)
                    .await?;
                Ok::<RunSummary, Error>(result.summary)
            });
        }

        let mut summaries = Vec::new();
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(Ok(summary)) => summaries.push(summary),
                Ok(Err(e)) => return Err(e),
                Err(e) => return Err(Error::Other(format!("task join error: {e}"))),
            }
        }

        Ok(EngineResult { summaries })
    }
}
