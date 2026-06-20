pub mod ssh;

pub use ssh::HostConn;
pub use ssh::HostKeyChecking;

use std::collections::HashMap;
use std::future::Future;

use crate::bundle::Bundle;
use crate::error::Error;
use crate::resources::RunSummary;

pub struct ExecResult {
    pub summary: RunSummary,
}

pub trait Transport: Send + Sync {
    /// A fresh per-host instance. SSH uses a per-host ControlMaster directory;
    /// each call to `for_host` creates an independent control socket path.
    fn for_host(&self) -> Self
    where
        Self: Sized;

    /// The agent version this transport ships. The engine compares this to the
    /// remote version stamp to decide whether an agent push is needed.
    fn current_version(&self) -> &str;

    fn preflight(
        &self,
        conn: &HostConn<'_>,
    ) -> impl Future<Output = Result<(HashMap<String, String>, Option<String>), Error>> + Send;

    fn execute(
        &self,
        conn: &HostConn<'_>,
        bundle: &Bundle,
        dry_run: bool,
        arch: &str,
        has_version: bool,
    ) -> impl Future<Output = Result<ExecResult, Error>> + Send;

    fn teardown_control_master(&self, conn: &HostConn<'_>);
}
