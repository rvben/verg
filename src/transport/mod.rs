pub mod ssh;

pub use ssh::HostKeyChecking;

use crate::resources::RunSummary;

pub struct ExecResult {
    pub summary: RunSummary,
}
