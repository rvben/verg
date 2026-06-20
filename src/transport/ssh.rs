use std::path::PathBuf;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::bundle::Bundle;
use crate::error::Error;
use crate::resources::RunSummary;

use super::ExecResult;

const AGENT_PATH: &str = "/usr/local/bin/verg-agent";
const VERSION_PATH: &str = "/usr/local/share/verg/version";

/// SSH host key checking policy passed to StrictHostKeyChecking.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum HostKeyChecking {
    /// Host must already be in known_hosts; a changed key is rejected.
    Yes,
    /// Trust on first use: accept an unknown host, reject a changed key.
    AcceptNew,
    /// Disable host key checking (unsafe).
    No,
}

impl HostKeyChecking {
    fn as_ssh_value(self) -> &'static str {
        match self {
            HostKeyChecking::Yes => "yes",
            HostKeyChecking::AcceptNew => "accept-new",
            HostKeyChecking::No => "no",
        }
    }
}

/// Compute the lowercase hex SHA-256 of a local file via `sha256sum`.
fn sha256_file(path: &std::path::Path) -> Result<String, Error> {
    let output = std::process::Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| Error::Other(format!("sha256sum: {e}")))?;
    if !output.status.success() {
        return Err(Error::Other(format!(
            "sha256sum failed for {}",
            path.display()
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let hash = stdout.split_whitespace().next().unwrap_or("").to_string();
    if hash.len() != 64 {
        return Err(Error::Other(format!(
            "unexpected sha256sum output for {}",
            path.display()
        )));
    }
    Ok(hash)
}

/// Decide whether the agent must be (re)pushed: push if the version is absent,
/// or if an expected hash is known and the installed remote hash differs.
fn should_push(has_version: bool, expected: Option<&str>, remote_hash: &str) -> bool {
    if !has_version {
        return true;
    }
    match expected {
        Some(h) => remote_hash != h,
        None => false,
    }
}

pub struct SshTransport {
    pub agent_dir: PathBuf,
    pub version: String,
    pub ssh_config: Option<PathBuf>,
    pub host_key_checking: HostKeyChecking,
    pub known_hosts: Option<PathBuf>,
    pub skip_agent_checksum: bool,
}

impl SshTransport {
    pub fn new(agent_dir: PathBuf, version: String) -> Self {
        Self {
            agent_dir,
            version,
            ssh_config: None,
            host_key_checking: HostKeyChecking::Yes,
            known_hosts: None,
            skip_agent_checksum: false,
        }
    }

    fn ssh_base_args(&self) -> Vec<String> {
        let mut args = vec![
            "-o".into(),
            "BatchMode=yes".into(),
            "-o".into(),
            format!(
                "StrictHostKeyChecking={}",
                self.host_key_checking.as_ssh_value()
            ),
        ];
        if let Some(file) = &self.known_hosts {
            args.push("-o".into());
            args.push(format!("UserKnownHostsFile={}", file.to_string_lossy()));
        }
        if let Some(config) = &self.ssh_config {
            args.push("-F".into());
            args.push(config.to_string_lossy().into_owned());
        }
        args
    }

    /// Gather basic system facts from the target in a single SSH command.
    /// Returns a HashMap with keys like "fact.arch", "fact.hostname", etc.
    pub async fn gather_facts(
        &self,
        user: &str,
        address: &str,
        port: Option<u16>,
    ) -> Result<std::collections::HashMap<String, String>, Error> {
        let target = format!("{user}@{address}");
        let mut args = self.ssh_base_args();
        if let Some(p) = port {
            args.extend(["-p".into(), p.to_string()]);
        }
        args.extend(["-o".into(), "ConnectTimeout=10".into(), target]);
        args.push(
            "echo \"arch=$(uname -m)\" && \
             echo \"hostname=$(hostname)\" && \
             echo \"os=$(. /etc/os-release 2>/dev/null && echo $ID)\" && \
             echo \"os_release=$(. /etc/os-release 2>/dev/null && echo $VERSION_CODENAME)\" && \
             echo \"os_version=$(. /etc/os-release 2>/dev/null && echo $VERSION_ID)\""
                .into(),
        );

        let output = Command::new("ssh")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Connection(format!("ssh facts: {e}")))?;

        if !output.status.success() {
            return Err(Error::Connection(
                "failed to gather facts from target".into(),
            ));
        }

        let mut facts = std::collections::HashMap::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            if let Some((key, val)) = line.split_once('=') {
                facts.insert(format!("fact.{key}"), val.to_string());
            }
        }

        Ok(facts)
    }

    fn arch_to_target(arch: &str) -> Result<&'static str, Error> {
        match arch {
            "x86_64" => Ok("x86_64-unknown-linux-gnu"),
            "aarch64" => Ok("aarch64-unknown-linux-gnu"),
            other => Err(Error::Config(format!(
                "unsupported target architecture: {other}"
            ))),
        }
    }

    fn verify_local_agent(&self, path: &std::path::Path, target: &str) -> Result<(), Error> {
        if self.skip_agent_checksum {
            return Ok(());
        }
        let Some(expected) = crate::agent_checksums::expected_sha256(target) else {
            // No embedded manifest (dev build): nothing to verify against.
            return Ok(());
        };
        let actual = sha256_file(path)?;
        if actual != expected {
            return Err(Error::Config(format!(
                "agent binary checksum mismatch for {target}: expected {expected}, got {actual}. \
                 Refusing to push a tampered or corrupt binary. Use --skip-agent-checksum to override."
            )));
        }
        Ok(())
    }

    async fn agent_binary_for_arch(&self, arch: &str) -> Result<PathBuf, Error> {
        let target = Self::arch_to_target(arch)?;
        let version_dir = self.agent_dir.join(&self.version);
        let cached = version_dir.join(format!("verg-agent-{target}"));

        // Check versioned cache first
        if cached.exists() {
            self.verify_local_agent(&cached, target)?;
            return Ok(cached);
        }

        // Download from GitHub releases
        eprintln!("Downloading verg-agent v{} for {target}...", self.version);
        let url = format!(
            "https://github.com/rvben/verg/releases/download/v{}/verg-agent-{target}",
            self.version
        );

        std::fs::create_dir_all(&version_dir).map_err(|e| {
            Error::Config(format!(
                "failed to create agents dir {}: {e}",
                version_dir.display()
            ))
        })?;

        let output = Command::new("curl")
            .args(["-fSL", "--progress-bar", "-o"])
            .arg(&cached)
            .arg(&url)
            .output()
            .await
            .map_err(|e| Error::Connection(format!("curl: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Clean up partial download
            let _ = std::fs::remove_file(&cached);
            return Err(Error::Config(format!(
                "failed to download agent binary from {url}\n{stderr}\n\
                 Hint: the release v{} may not exist yet. \
                 Build locally with `cargo build --release --target {target} --bin verg-agent`",
                self.version
            )));
        }

        // Make executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&cached, std::fs::Permissions::from_mode(0o755))
                .map_err(|e| Error::Config(format!("failed to chmod agent binary: {e}")))?;
        }

        // Clean up old versions
        if let Ok(entries) = std::fs::read_dir(&self.agent_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path != version_dir {
                    let _ = std::fs::remove_dir_all(&path);
                }
            }
        }

        eprintln!("Cached at {}", cached.display());
        self.verify_local_agent(&cached, target)?;
        Ok(cached)
    }

    async fn check_version(
        &self,
        user: &str,
        address: &str,
        port: Option<u16>,
    ) -> Result<bool, Error> {
        let target = format!("{user}@{address}");
        let mut args = self.ssh_base_args();
        if let Some(p) = port {
            args.extend(["-p".into(), p.to_string()]);
        }
        args.extend(["-o".into(), "ConnectTimeout=10".into(), target]);
        args.push(format!("cat {VERSION_PATH} 2>/dev/null"));

        let output = Command::new("ssh")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Connection(format!("ssh to {address}: {e}")))?;

        let remote_version = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(remote_version == self.version)
    }

    async fn remote_agent_sha256(
        &self,
        user: &str,
        address: &str,
        port: Option<u16>,
    ) -> Result<String, Error> {
        let target = format!("{user}@{address}");
        let mut args = self.ssh_base_args();
        if let Some(p) = port {
            args.extend(["-p".into(), p.to_string()]);
        }
        args.extend([
            "-o".into(),
            "ConnectTimeout=10".into(),
            target,
            format!("sha256sum {AGENT_PATH} 2>/dev/null || true"),
        ]);
        let output = Command::new("ssh")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Connection(format!("ssh to {address}: {e}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.split_whitespace().next().unwrap_or("").to_string())
    }

    async fn push_binary(
        &self,
        user: &str,
        address: &str,
        port: Option<u16>,
        agent_binary: &std::path::Path,
        expected: Option<&str>,
    ) -> Result<(), Error> {
        let target = format!("{user}@{address}");

        // Create directories
        let mut args = self.ssh_base_args();
        if let Some(p) = port {
            args.extend(["-p".into(), p.to_string()]);
        }
        args.extend([
            target.clone(),
            "mkdir -p /usr/local/bin /usr/local/share/verg".into(),
        ]);
        let output = Command::new("ssh")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Connection(format!("ssh mkdir: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Connection(format!(
                "failed to create dirs: {stderr}"
            )));
        }

        // Copy to a temp path beside the final binary (so mv is an atomic rename).
        let tmp_remote = format!("{AGENT_PATH}.tmp.{}", std::process::id());
        let mut scp_args = self.ssh_base_args();
        if let Some(p) = port {
            scp_args.extend(["-P".into(), p.to_string()]);
        }
        scp_args.push(agent_binary.to_string_lossy().into_owned());
        scp_args.push(format!("{target}:{tmp_remote}"));
        let output = Command::new("scp")
            .args(&scp_args)
            .output()
            .await
            .map_err(|e| Error::Connection(format!("scp: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Connection(format!(
                "failed to push binary: {stderr}"
            )));
        }

        // Verify the pushed temp file against the expected hash (when known),
        // then atomically install with mode 0700 and write the version file. On
        // any failure the temp file is removed so a bad copy is never trusted.

        // Guard: reject a malformed embedded checksum before it reaches shell interpolation.
        if let Some(hash) = expected
            && (hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            return Err(Error::Other(format!(
                "refusing to install agent: embedded checksum is not a valid sha256: {hash}"
            )));
        }

        let verify = match expected {
            Some(hash) => format!(
                "printf '%s  %s\\n' '{hash}' '{tmp_remote}' | sha256sum -c - >/dev/null && "
            ),
            None => String::new(),
        };
        let install_cmd = format!(
            "{verify}chmod 700 '{tmp_remote}' && mv '{tmp_remote}' {AGENT_PATH} && \
             printf '%s' '{}' > {VERSION_PATH} || {{ rm -f '{tmp_remote}'; exit 1; }}",
            self.version
        );
        let mut args = self.ssh_base_args();
        if let Some(p) = port {
            args.extend(["-p".into(), p.to_string()]);
        }
        args.extend([target, install_cmd]);
        let output = Command::new("ssh")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Connection(format!("ssh install: {e}")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::Connection(format!(
                "failed to verify/install agent (checksum mismatch or install error): {stderr}"
            )));
        }

        Ok(())
    }

    pub async fn execute(
        &self,
        user: &str,
        address: &str,
        port: Option<u16>,
        bundle: &Bundle,
        dry_run: bool,
    ) -> Result<ExecResult, Error> {
        let facts = self.gather_facts(user, address, port).await?;
        let arch = facts
            .get("fact.arch")
            .cloned()
            .unwrap_or_else(|| "x86_64".into());

        let has_version = self.check_version(user, address, port).await?;
        let arch_target = Self::arch_to_target(&arch)?;
        let expected = if self.skip_agent_checksum {
            None
        } else {
            crate::agent_checksums::expected_sha256(arch_target)
        };
        let needs_push = if has_version && expected.is_some() {
            let remote = self.remote_agent_sha256(user, address, port).await?;
            should_push(true, expected, &remote)
        } else {
            should_push(has_version, expected, "")
        };
        if needs_push {
            let agent_binary = self.agent_binary_for_arch(&arch).await?;
            self.push_binary(user, address, port, &agent_binary, expected)
                .await?;
        }

        let target = format!("{user}@{address}");
        let bundle_toml = bundle.to_toml()?;

        let mut cmd_str = AGENT_PATH.to_string();
        if dry_run {
            cmd_str.push_str(" --dry-run");
        }

        let mut args = self.ssh_base_args();
        if let Some(p) = port {
            args.extend(["-p".into(), p.to_string()]);
        }
        args.extend([target, cmd_str]);

        let mut child = Command::new("ssh")
            .args(&args)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_args_default_is_strict_yes() {
        let t = SshTransport::new(std::path::PathBuf::from("/tmp"), "0.0.0".into());
        let args = t.ssh_base_args();
        let joined = args.join(" ");
        assert!(
            joined.contains("StrictHostKeyChecking=yes"),
            "got: {joined}"
        );
        assert!(joined.contains("BatchMode=yes"), "got: {joined}");
    }

    #[test]
    fn base_args_accept_new_and_known_hosts() {
        let mut t = SshTransport::new(std::path::PathBuf::from("/tmp"), "0.0.0".into());
        t.host_key_checking = HostKeyChecking::AcceptNew;
        t.known_hosts = Some(std::path::PathBuf::from("/etc/verg/known_hosts"));
        let joined = t.ssh_base_args().join(" ");
        assert!(
            joined.contains("StrictHostKeyChecking=accept-new"),
            "got: {joined}"
        );
        assert!(
            joined.contains("UserKnownHostsFile=/etc/verg/known_hosts"),
            "got: {joined}"
        );
    }

    #[test]
    fn sha256_file_matches_known_vector() {
        // SHA-256 of the empty file.
        let f = tempfile::NamedTempFile::new().unwrap();
        let hash = sha256_file(f.path()).unwrap();
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn should_push_decision() {
        assert!(super::should_push(false, None, "")); // no version -> push
        assert!(super::should_push(false, Some("a"), "a")); // no version -> push regardless
        assert!(!super::should_push(true, None, "")); // version ok, no checksum -> skip
        assert!(!super::should_push(true, Some("a"), "a")); // installed matches -> skip
        assert!(super::should_push(true, Some("a"), "b")); // mismatch -> repush
        assert!(super::should_push(true, Some("a"), "")); // remote absent -> repush
    }
}
