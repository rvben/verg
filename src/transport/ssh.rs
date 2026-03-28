use std::path::PathBuf;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::bundle::Bundle;
use crate::error::Error;
use crate::resources::RunSummary;

use super::ExecResult;

const AGENT_PATH: &str = "/usr/local/bin/verg-agent";
const VERSION_PATH: &str = "/usr/local/share/verg/version";

pub struct SshTransport {
    pub agent_binary: PathBuf,
    pub version: String,
}

impl SshTransport {
    pub fn new(agent_binary: PathBuf, version: String) -> Self {
        Self {
            agent_binary,
            version,
        }
    }

    async fn check_version(&self, user: &str, address: &str) -> Result<bool, Error> {
        let target = format!("{user}@{address}");
        let output = Command::new("ssh")
            .args(["-o", "BatchMode=yes", "-o", "ConnectTimeout=10", &target])
            .arg(format!("cat {VERSION_PATH} 2>/dev/null"))
            .output()
            .await
            .map_err(|e| Error::Connection(format!("ssh to {address}: {e}")))?;

        let remote_version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(remote_version == self.version)
    }

    async fn push_binary(&self, user: &str, address: &str) -> Result<(), Error> {
        let target = format!("{user}@{address}");

        let output = Command::new("ssh")
            .args(["-o", "BatchMode=yes", &target])
            .arg("mkdir -p /usr/local/bin /usr/local/share/verg")
            .output()
            .await
            .map_err(|e| Error::Connection(format!("ssh mkdir: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Connection(format!(
                "failed to create dirs: {stderr}"
            )));
        }

        let output = Command::new("scp")
            .args(["-o", "BatchMode=yes"])
            .arg(self.agent_binary.to_str().unwrap())
            .arg(format!("{target}:{AGENT_PATH}"))
            .output()
            .await
            .map_err(|e| Error::Connection(format!("scp: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Connection(format!(
                "failed to push binary: {stderr}"
            )));
        }

        let output = Command::new("ssh")
            .args(["-o", "BatchMode=yes", &target])
            .arg(format!(
                "chmod +x {AGENT_PATH} && echo '{}' > {VERSION_PATH}",
                self.version
            ))
            .output()
            .await
            .map_err(|e| Error::Connection(format!("ssh chmod: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Connection(format!(
                "failed to set up agent: {stderr}"
            )));
        }

        Ok(())
    }

    pub async fn execute(
        &self,
        user: &str,
        address: &str,
        bundle: &Bundle,
        dry_run: bool,
    ) -> Result<ExecResult, Error> {
        let has_version = self.check_version(user, address).await?;
        if !has_version {
            self.push_binary(user, address).await?;
        }

        let target = format!("{user}@{address}");
        let bundle_toml = bundle.to_toml()?;

        let mut cmd_str = AGENT_PATH.to_string();
        if dry_run {
            cmd_str.push_str(" --dry-run");
        }

        let mut child = Command::new("ssh")
            .args(["-o", "BatchMode=yes", &target, &cmd_str])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| Error::Connection(format!("ssh spawn: {e}")))?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(bundle_toml.as_bytes())
                .await
                .map_err(|e| Error::Connection(format!("write to ssh stdin: {e}")))?;
            drop(stdin);
        }

        let output = child
            .wait_with_output()
            .await
            .map_err(|e| Error::Connection(format!("ssh wait: {e}")))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let summary: RunSummary = serde_json::from_str(&stdout).map_err(|e| {
            Error::Other(format!(
                "failed to parse agent output: {e}\nraw output: {stdout}"
            ))
        })?;

        Ok(ExecResult { summary })
    }
}
