use std::path::PathBuf;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::bundle::Bundle;
use crate::error::Error;
use crate::resources::RunSummary;

use super::ExecResult;

const AGENT_PATH: &str = "/usr/local/bin/verg-agent";

/// Bound a possibly-huge string for inclusion in an error message.
fn truncate_for_error(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... ({} bytes total)", &s[..end], s.len())
}
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

/// Parse the output of the combined preflight SSH command.
///
/// Each line is `key=value`. Keys `arch`, `hostname`, `os`, `os_release`, and
/// `os_version` are inserted into the facts map with a `fact.` prefix (matching
/// the former `gather_facts` output). The `version` key is returned separately
/// as the raw version string (may be empty when no agent is installed yet).
pub fn parse_preflight(
    stdout: &str,
) -> (std::collections::HashMap<String, String>, Option<String>) {
    let mut facts = std::collections::HashMap::new();
    let mut version: Option<String> = None;
    for line in stdout.lines() {
        if let Some((key, val)) = line.split_once('=') {
            if key == "version" {
                version = Some(val.to_string());
            } else {
                facts.insert(format!("fact.{key}"), val.to_string());
            }
        }
    }
    (facts, version)
}

/// True when the remote version stamp (trimmed) matches the running verg version.
/// A missing, empty, or older stamp returns false, triggering an agent push.
pub fn version_matches(remote: &str, current: &str) -> bool {
    remote.trim() == current
}

/// SSH connection coordinates for a single host target.
pub struct HostConn<'a> {
    pub user: &'a str,
    pub address: &'a str,
    pub port: Option<u16>,
}

pub struct SshTransport {
    pub agent_dir: PathBuf,
    pub version: String,
    pub ssh_config: Option<PathBuf>,
    pub host_key_checking: HostKeyChecking,
    pub known_hosts: Option<PathBuf>,
    pub skip_agent_checksum: bool,
    /// Per-transport directory holding the ControlMaster socket (`%C` hash).
    /// Scoped to this process and host; used by both ssh and scp invocations.
    control_dir: PathBuf,
}

impl SshTransport {
    pub fn new(agent_dir: PathBuf, version: String) -> Self {
        // Unique per transport instance (one per host) so each host's teardown
        // removes only its own socket directory and concurrent hosts never race
        // on the same path. Rooted at a SHORT base (/tmp on unix) because the
        // fully expanded ControlPath (`<dir>/%C`, where %C is a 40-char hash)
        // must stay under the unix-domain socket path limit (~104 bytes on
        // macOS); the default temp dir on macOS (/var/folders/...) is already
        // long enough to overflow that with a per-host suffix.
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let base = if cfg!(unix) {
            PathBuf::from("/tmp")
        } else {
            std::env::temp_dir()
        };
        let control_dir = base.join(format!("vcm-{}-{}", std::process::id(), seq));
        // Best-effort: if the dir cannot be created, ControlMaster=auto will
        // warn and fall back to a direct connection, so no error is fatal here.
        let _ = std::fs::create_dir_all(&control_dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&control_dir, std::fs::Permissions::from_mode(0o700));
        }
        Self {
            agent_dir,
            version,
            ssh_config: None,
            host_key_checking: HostKeyChecking::Yes,
            known_hosts: None,
            skip_agent_checksum: false,
            control_dir,
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
        args.extend([
            "-o".into(),
            "ConnectTimeout=10".into(),
            "-o".into(),
            "ServerAliveInterval=15".into(),
            "-o".into(),
            "ServerAliveCountMax=3".into(),
        ]);
        // Connection multiplexing: reuse one TCP+auth session per host across
        // all ssh/scp invocations. ControlMaster=auto is fail-safe: if the
        // socket cannot be created, ssh warns and connects normally.
        // %C is OpenSSH's short connection hash (avoids long path collisions on
        // macOS where unix socket paths are capped near 104 bytes).
        args.extend([
            "-o".into(),
            "ControlMaster=auto".into(),
            "-o".into(),
            format!("ControlPath={}/{}", self.control_dir.display(), "%C"),
            "-o".into(),
            "ControlPersist=60s".into(),
        ]);
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

    /// Returns the control-master args needed to send a control command
    /// (e.g. `-O exit`) to an existing master socket for this transport.
    fn control_master_args(&self) -> Vec<String> {
        vec![
            "-o".into(),
            "ControlMaster=auto".into(),
            "-o".into(),
            format!("ControlPath={}/{}", self.control_dir.display(), "%C"),
        ]
    }

    /// Best-effort teardown of the ControlMaster for a given host target
    /// (`user@address [-p port]`). Called after all work for that host is done.
    /// Errors are silently ignored; this must never affect the host result.
    pub fn teardown_control_master(&self, conn: &HostConn<'_>) {
        let target = format!("{}@{}", conn.user, conn.address);
        let mut args = self.control_master_args();
        if let Some(p) = conn.port {
            args.extend(["-p".into(), p.to_string()]);
        }
        // -O exit signals the master to shut down cleanly. Runs synchronously
        // (no network involved - it writes to the local unix socket) so there
        // is no meaningful latency cost.
        args.extend(["-O".into(), "exit".into(), target]);
        let _ = std::process::Command::new("ssh").args(&args).output();
        let _ = std::fs::remove_dir_all(&self.control_dir);
    }

    /// Collect system facts and the installed agent version in a single SSH
    /// round-trip. Returns the parsed facts map (keys prefixed `fact.*`) and
    /// the raw version string (empty when no agent is installed).
    ///
    /// The fact-producing portion of the remote command is byte-identical to
    /// the former `gather_facts` command so fact values are unchanged.
    pub async fn preflight(
        &self,
        conn: &HostConn<'_>,
    ) -> Result<(std::collections::HashMap<String, String>, Option<String>), Error> {
        let target = format!("{}@{}", conn.user, conn.address);
        let mut args = self.ssh_base_args();
        if let Some(p) = conn.port {
            args.extend(["-p".into(), p.to_string()]);
        }
        args.push(target);
        args.push(
            "echo \"arch=$(uname -m)\" && \
             echo \"hostname=$(hostname)\" && \
             echo \"os=$(. /etc/os-release 2>/dev/null && echo $ID)\" && \
             echo \"os_release=$(. /etc/os-release 2>/dev/null && echo $VERSION_CODENAME)\" && \
             echo \"os_version=$(. /etc/os-release 2>/dev/null && echo $VERSION_ID)\" && \
             echo \"version=$(cat /usr/local/share/verg/version 2>/dev/null)\""
                .into(),
        );

        let output = Command::new("ssh")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Connection(format!("ssh preflight: {e}")))?;

        if !output.status.success() {
            return Err(Error::Connection(
                "failed to run preflight on target".into(),
            ));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(parse_preflight(&stdout))
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
            .args(["-fSL", "--progress-bar", "-m", "300", "-o"])
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

    async fn remote_agent_sha256(&self, conn: &HostConn<'_>) -> Result<String, Error> {
        let target = format!("{}@{}", conn.user, conn.address);
        let mut args = self.ssh_base_args();
        if let Some(p) = conn.port {
            args.extend(["-p".into(), p.to_string()]);
        }
        args.extend([
            target,
            format!("sha256sum {AGENT_PATH} 2>/dev/null || true"),
        ]);
        let output = Command::new("ssh")
            .args(&args)
            .output()
            .await
            .map_err(|e| Error::Connection(format!("ssh to {}: {e}", conn.address)))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        Ok(stdout.split_whitespace().next().unwrap_or("").to_string())
    }

    async fn push_binary(
        &self,
        conn: &HostConn<'_>,
        agent_binary: &std::path::Path,
        expected: Option<&str>,
    ) -> Result<(), Error> {
        let target = format!("{}@{}", conn.user, conn.address);

        // Create directories
        let mut args = self.ssh_base_args();
        if let Some(p) = conn.port {
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
        if let Some(p) = conn.port {
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
        if let Some(p) = conn.port {
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
        conn: &HostConn<'_>,
        bundle: &Bundle,
        dry_run: bool,
        arch: &str,
        has_version: bool,
    ) -> Result<ExecResult, Error> {
        let arch_target = Self::arch_to_target(arch)?;
        let expected = if self.skip_agent_checksum {
            None
        } else {
            crate::agent_checksums::expected_sha256(arch_target)
        };
        let needs_push = if has_version && expected.is_some() {
            let remote = self.remote_agent_sha256(conn).await?;
            should_push(true, expected, &remote)
        } else {
            should_push(has_version, expected, "")
        };
        if needs_push {
            let agent_binary = self.agent_binary_for_arch(arch).await?;
            self.push_binary(conn, &agent_binary, expected).await?;
        }

        let target = format!("{}@{}", conn.user, conn.address);
        let bundle_toml = bundle.to_toml()?;

        let mut cmd_str = AGENT_PATH.to_string();
        if dry_run {
            cmd_str.push_str(" --dry-run");
        }

        let mut args = self.ssh_base_args();
        if let Some(p) = conn.port {
            args.extend(["-p".into(), p.to_string()]);
        }
        args.extend([target, cmd_str]);

        let mut child = Command::new("ssh")
            .args(&args)
            .kill_on_drop(true)
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
                "failed to parse agent output: {e}\nraw output: {}",
                truncate_for_error(&stdout, 512)
            ))
        })?;

        Ok(ExecResult { summary })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_preflight_extracts_facts_and_version() {
        let out = "arch=x86_64\nhostname=web1\nos=ubuntu\nos_release=jammy\nos_version=22.04\nversion=0.6.5\n";
        let (facts, version) = parse_preflight(out);
        assert_eq!(facts.get("fact.arch").map(String::as_str), Some("x86_64"));
        assert_eq!(facts.get("fact.hostname").map(String::as_str), Some("web1"));
        assert_eq!(facts.get("fact.os").map(String::as_str), Some("ubuntu"));
        assert_eq!(
            facts.get("fact.os_release").map(String::as_str),
            Some("jammy")
        );
        assert_eq!(
            facts.get("fact.os_version").map(String::as_str),
            Some("22.04")
        );
        assert_eq!(version.as_deref(), Some("0.6.5"));
    }

    #[test]
    fn parse_preflight_empty_version_is_none_or_empty() {
        let out =
            "arch=x86_64\nhostname=web1\nos=ubuntu\nos_release=jammy\nos_version=22.04\nversion=\n";
        let (_facts, version) = parse_preflight(out);
        assert!(version.as_deref().unwrap_or("").is_empty());
    }

    #[test]
    fn version_matches_only_on_exact_current_version() {
        assert!(version_matches("0.6.5", "0.6.5"));
        assert!(!version_matches("", "0.6.5")); // fresh host
        assert!(!version_matches("0.6.4", "0.6.5")); // stale agent
        assert!(version_matches(" 0.6.5\n", "0.6.5")); // trimmed
    }

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

    #[test]
    fn base_args_include_timeout_options() {
        let t = SshTransport::new(std::path::PathBuf::from("/tmp"), "0.0.0".into());
        let joined = t.ssh_base_args().join(" ");
        assert!(joined.contains("ConnectTimeout=10"), "got: {joined}");
        assert!(joined.contains("ServerAliveInterval=15"), "got: {joined}");
        assert!(joined.contains("ServerAliveCountMax=3"), "got: {joined}");
    }

    #[test]
    fn base_args_enable_connection_multiplexing() {
        let t = SshTransport::new(std::path::PathBuf::from("/tmp"), "0.0.0".into());
        let joined = t.ssh_base_args().join(" ");
        assert!(joined.contains("ControlMaster=auto"), "got: {joined}");
        assert!(joined.contains("ControlPath="), "got: {joined}");
        // %C is the short connection hash; a literal hostname token (%h) could
        // blow the macOS unix-socket path limit, so pin the token.
        assert!(joined.contains("/%C"), "got: {joined}");
        assert!(joined.contains("ControlPersist="), "got: {joined}");
    }

    #[test]
    fn truncate_for_error_bounds_output() {
        assert_eq!(truncate_for_error("short", 512), "short");
        let big = "x".repeat(2000);
        let out = truncate_for_error(&big, 512);
        assert!(out.len() < big.len(), "should be truncated");
        assert!(out.contains("2000 bytes total"), "got: {out}");
    }

    #[test]
    fn truncate_for_error_respects_char_boundary() {
        // 10 ASCII bytes then a 3-byte char; max=11 lands INSIDE that char.
        // A naive &s[..11] would panic; the helper must back off to byte 10.
        let s = "a".repeat(10) + "\u{20AC}"; // euro sign, 3 bytes (bytes 10..13)
        let out = truncate_for_error(&s, 11);
        // Must not panic, and the kept prefix is exactly the 10 'a's.
        assert!(out.starts_with(&"a".repeat(10)));
        assert!(
            !out.contains('\u{20AC}'),
            "the split multibyte char must be dropped"
        );
    }
}
