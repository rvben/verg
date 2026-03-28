pub mod ssh;

use crate::resources::RunSummary;

pub struct ExecResult {
    pub summary: RunSummary,
}
