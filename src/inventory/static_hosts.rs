use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::Error;

#[derive(Debug, Clone, Deserialize)]
pub struct HostDef {
    pub address: String,
    #[serde(default = "default_user")]
    pub user: String,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub groups: Vec<String>,
    #[serde(default)]
    pub vars: HashMap<String, toml::Value>,
}

fn default_user() -> String {
    "root".into()
}

#[derive(Debug, Clone, Deserialize)]
pub struct InventoryConfig {
    pub command: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct HostsFile {
    #[serde(default)]
    hosts: HashMap<String, HostDef>,
    #[serde(default)]
    inventory: Option<InventoryConfig>,
}

/// Parsed `hosts.toml`: the static host table plus an optional dynamic-inventory command.
pub struct ParsedHosts {
    pub hosts: HashMap<String, HostDef>,
    pub inventory: Option<InventoryConfig>,
}

/// Reject host fields that could inject ssh options or shell metacharacters.
pub fn validate_host_field(label: &str, value: &str) -> Result<(), Error> {
    if value.starts_with('-') {
        return Err(Error::Config(format!(
            "{label} '{value}' must not start with '-' (would be parsed as an ssh option)"
        )));
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '@' | '-' | '[' | ']'))
    {
        return Err(Error::Config(format!(
            "{label} '{value}' contains invalid characters (allowed: alphanumerics . _ : @ - [ ])"
        )));
    }
    Ok(())
}

pub fn load_hosts(path: &Path) -> Result<ParsedHosts, Error> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| Error::Config(format!("failed to read {}: {e}", path.display())))?;
    let file: HostsFile = toml::from_str(&content)
        .map_err(|e| Error::Parse(format!("failed to parse {}: {e}", path.display())))?;
    for (name, def) in &file.hosts {
        validate_host_field("user", &def.user)
            .map_err(|e| Error::Config(format!("host '{name}': {e}")))?;
        validate_host_field("address", &def.address)
            .map_err(|e| Error::Config(format!("host '{name}': {e}")))?;
    }
    Ok(ParsedHosts {
        hosts: file.hosts,
        inventory: file.inventory,
    })
}

/// Runs the dynamic-inventory command on the control host and parses its JSON
/// stdout into host definitions.
///
/// The command is an argv vector executed with no shell, with its working
/// directory set to `base_dir` (the directory containing `hosts.toml`), so a
/// relative command path resolves against the verg config directory. Its stdout
/// must be a JSON object mapping host name to the same fields a static
/// `[hosts.NAME]` table accepts (`address` required; `user`, `port`, `groups`,
/// `vars` optional). `vars` values must be representable as TOML values (JSON
/// `null` is rejected, since TOML has no null). Each returned host is validated
/// with the same checks as static hosts.
/// Linux/macOS errno for ETXTBSY ("text file busy"): the program file is open
/// for writing somewhere.
const ETXTBSY: i32 = 26;

/// Run the command, retrying briefly on ETXTBSY. ETXTBSY happens when the program
/// file is still open for writing - an inventory script being rewritten
/// concurrently, or, in a multithreaded process, briefly held open by another
/// thread's `fork` before its `exec` (the classic fork/exec race). It is
/// transient, so a short bounded retry resolves it instead of failing the run.
fn spawn_with_etxtbsy_retry(
    program: &str,
    args: &[String],
    base_dir: &Path,
) -> std::io::Result<std::process::Output> {
    const MAX_ATTEMPTS: u32 = 5;
    let mut attempt = 0;
    loop {
        match std::process::Command::new(program)
            .args(args)
            .current_dir(base_dir)
            .output()
        {
            Err(e) if e.raw_os_error() == Some(ETXTBSY) && attempt + 1 < MAX_ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(20 * u64::from(attempt)));
            }
            other => return other,
        }
    }
}

pub fn run_inventory_command(
    cfg: &InventoryConfig,
    base_dir: &Path,
) -> Result<HashMap<String, HostDef>, Error> {
    let Some((program, args)) = cfg.command.split_first() else {
        return Err(Error::Config(
            "inventory command is empty (set [inventory].command to a non-empty argv array)".into(),
        ));
    };

    let output = spawn_with_etxtbsy_retry(program, args, base_dir)
        .map_err(|e| Error::Config(format!("failed to run inventory command '{program}': {e}")))?;

    if !output.status.success() {
        return Err(Error::Config(format!(
            "inventory command '{program}' failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    let stdout = String::from_utf8(output.stdout)
        .map_err(|e| Error::Parse(format!("inventory command output is not valid UTF-8: {e}")))?;

    let hosts: HashMap<String, HostDef> = serde_json::from_str(&stdout).map_err(|e| {
        Error::Parse(format!(
            "inventory command output is not a valid JSON host map: {e}"
        ))
    })?;

    for (name, def) in &hosts {
        validate_host_field("user", &def.user)
            .map_err(|e| Error::Config(format!("dynamic host '{name}': {e}")))?;
        validate_host_field("address", &def.address)
            .map_err(|e| Error::Config(format!("dynamic host '{name}': {e}")))?;
    }

    Ok(hosts)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    #[test]
    fn parse_hosts_file() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"
[hosts.web1]
address = "192.0.2.10"
user = "root"
groups = ["web", "prod"]

[hosts.web2]
address = "192.0.2.11"
groups = ["web"]
"#
        )
        .unwrap();

        let hosts = load_hosts(f.path()).unwrap().hosts;
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts["web1"].address, "192.0.2.10");
        assert_eq!(hosts["web1"].user, "root");
        assert_eq!(hosts["web1"].groups, vec!["web", "prod"]);
        assert_eq!(hosts["web2"].user, "root"); // default
    }

    #[test]
    fn parse_hosts_with_vars() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"
[hosts.db1]
address = "192.0.2.5"
groups = ["db"]

[hosts.db1.vars]
port = 5432
data_dir = "/var/lib/postgres"
"#
        )
        .unwrap();

        let hosts = load_hosts(f.path()).unwrap().hosts;
        assert_eq!(hosts["db1"].vars["port"], toml::Value::Integer(5432));
    }

    #[test]
    fn parse_hosts_with_port() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"
[hosts.web1]
address = "192.0.2.10"
port = 2222
groups = ["web"]
"#
        )
        .unwrap();

        let hosts = load_hosts(f.path()).unwrap().hosts;
        assert_eq!(hosts["web1"].port, Some(2222));
    }

    #[test]
    fn port_defaults_to_none() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"
[hosts.web1]
address = "192.0.2.10"
"#
        )
        .unwrap();

        let hosts = load_hosts(f.path()).unwrap().hosts;
        assert_eq!(hosts["web1"].port, None);
    }

    #[test]
    fn missing_hosts_file_returns_config_error() {
        let result = load_hosts(Path::new("/nonexistent/hosts.toml"));
        assert!(matches!(result, Err(Error::Config(_))));
    }

    #[test]
    fn rejects_option_like_address() {
        assert!(validate_host_field("address", "-oProxyCommand=evil").is_err());
        assert!(validate_host_field("user", "root; rm -rf /").is_err());
        assert!(validate_host_field("address", "192.0.2.10").is_ok());
        assert!(validate_host_field("user", "deploy_user").is_ok());
    }

    #[test]
    fn accepts_ipv6_addresses() {
        assert!(validate_host_field("address", "::1").is_ok());
        assert!(validate_host_field("address", "[2001:db8::1]").is_ok());
        assert!(validate_host_field("address", "2001:db8::1").is_ok());
    }

    #[cfg(unix)]
    fn write_script(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("inventory.sh");
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn parses_inventory_section() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"
[hosts.web1]
address = "192.0.2.10"

[inventory]
command = ["/usr/bin/list-hosts", "--env", "prod"]
"#
        )
        .unwrap();

        let parsed = load_hosts(f.path()).unwrap();
        assert_eq!(parsed.hosts.len(), 1);
        let cfg = parsed.inventory.expect("inventory section present");
        assert_eq!(cfg.command, vec!["/usr/bin/list-hosts", "--env", "prod"]);
    }

    #[test]
    fn no_inventory_section_is_none() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"
[hosts.web1]
address = "192.0.2.10"
"#
        )
        .unwrap();

        let parsed = load_hosts(f.path()).unwrap();
        assert!(parsed.inventory.is_none());
    }

    #[test]
    fn inventory_only_file_parses_with_no_static_hosts() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"
[inventory]
command = ["/usr/bin/list-hosts"]
"#
        )
        .unwrap();

        let parsed = load_hosts(f.path()).unwrap();
        assert!(parsed.hosts.is_empty());
        assert!(parsed.inventory.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn run_inventory_command_parses_json_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(
            dir.path(),
            "#!/bin/sh\nprintf '%s' '{\"web1\":{\"address\":\"192.0.2.10\",\"user\":\"deploy\",\"port\":2222,\"groups\":[\"web\"],\"vars\":{\"role\":\"frontend\"}},\"db1\":{\"address\":\"192.0.2.5\"}}'\n",
        );
        let cfg = InventoryConfig {
            command: vec![script.to_string_lossy().into_owned()],
        };

        let hosts = run_inventory_command(&cfg, dir.path()).unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts["web1"].address, "192.0.2.10");
        assert_eq!(hosts["web1"].user, "deploy");
        assert_eq!(hosts["web1"].port, Some(2222));
        assert_eq!(hosts["web1"].groups, vec!["web"]);
        assert_eq!(
            hosts["web1"].vars["role"],
            toml::Value::String("frontend".into())
        );
        // Defaults apply to a minimal dynamic host, exactly as for static hosts.
        assert_eq!(hosts["db1"].user, "root");
        assert_eq!(hosts["db1"].port, None);
        assert!(hosts["db1"].groups.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn run_inventory_command_validates_host_fields() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(
            dir.path(),
            "#!/bin/sh\nprintf '%s' '{\"evil\":{\"address\":\"-oProxyCommand=evil\"}}'\n",
        );
        let cfg = InventoryConfig {
            command: vec![script.to_string_lossy().into_owned()],
        };

        let err = run_inventory_command(&cfg, dir.path()).unwrap_err();
        match err {
            Error::Config(msg) => assert!(msg.contains("evil"), "got: {msg}"),
            other => panic!("expected Error::Config, got: {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn run_inventory_command_nonzero_exit_is_config_error() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(dir.path(), "#!/bin/sh\necho 'boom' >&2\nexit 3\n");
        let cfg = InventoryConfig {
            command: vec![script.to_string_lossy().into_owned()],
        };

        let err = run_inventory_command(&cfg, dir.path()).unwrap_err();
        match err {
            Error::Config(msg) => assert!(msg.contains("boom"), "stderr should surface: {msg}"),
            other => panic!("expected Error::Config, got: {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn run_inventory_command_invalid_json_is_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(dir.path(), "#!/bin/sh\nprintf '%s' 'not json'\n");
        let cfg = InventoryConfig {
            command: vec![script.to_string_lossy().into_owned()],
        };

        let err = run_inventory_command(&cfg, dir.path()).unwrap_err();
        assert!(matches!(err, Error::Parse(_)), "got: {err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn run_inventory_command_json_null_var_is_parse_error() {
        // toml::Value has no null variant, so a JSON null in vars is rejected.
        // This documents that dynamic vars must be representable as TOML values.
        let dir = tempfile::tempdir().unwrap();
        let script = write_script(
            dir.path(),
            "#!/bin/sh\nprintf '%s' '{\"web1\":{\"address\":\"192.0.2.10\",\"vars\":{\"k\":null}}}'\n",
        );
        let cfg = InventoryConfig {
            command: vec![script.to_string_lossy().into_owned()],
        };

        let err = run_inventory_command(&cfg, dir.path()).unwrap_err();
        assert!(matches!(err, Error::Parse(_)), "got: {err:?}");
    }

    #[cfg(unix)]
    #[test]
    fn run_inventory_command_resolves_relative_path_from_base_dir() {
        // A relative command resolves against base_dir, not the process cwd.
        let dir = tempfile::tempdir().unwrap();
        write_script(
            dir.path(),
            "#!/bin/sh\nprintf '%s' '{\"web1\":{\"address\":\"192.0.2.10\"}}'\n",
        );
        let cfg = InventoryConfig {
            command: vec!["./inventory.sh".to_string()],
        };

        let hosts = run_inventory_command(&cfg, dir.path()).unwrap();
        assert_eq!(hosts["web1"].address, "192.0.2.10");
    }

    #[test]
    fn run_inventory_command_empty_command_is_config_error() {
        let cfg = InventoryConfig { command: vec![] };
        let err = run_inventory_command(&cfg, Path::new(".")).unwrap_err();
        assert!(matches!(err, Error::Config(_)), "got: {err:?}");
    }
}
