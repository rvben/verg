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

#[derive(Debug, Deserialize)]
struct HostsFile {
    hosts: HashMap<String, HostDef>,
}

pub fn load_hosts(path: &Path) -> Result<HashMap<String, HostDef>, Error> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| Error::Config(format!("failed to read {}: {e}", path.display())))?;
    let file: HostsFile = toml::from_str(&content)
        .map_err(|e| Error::Parse(format!("failed to parse {}: {e}", path.display())))?;
    Ok(file.hosts)
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
address = "192.168.1.10"
user = "root"
groups = ["web", "prod"]

[hosts.web2]
address = "192.168.1.11"
groups = ["web"]
"#
        )
        .unwrap();

        let hosts = load_hosts(f.path()).unwrap();
        assert_eq!(hosts.len(), 2);
        assert_eq!(hosts["web1"].address, "192.168.1.10");
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
address = "10.0.0.5"
groups = ["db"]

[hosts.db1.vars]
port = 5432
data_dir = "/var/lib/postgres"
"#
        )
        .unwrap();

        let hosts = load_hosts(f.path()).unwrap();
        assert_eq!(hosts["db1"].vars["port"], toml::Value::Integer(5432));
    }

    #[test]
    fn parse_hosts_with_port() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"
[hosts.web1]
address = "192.168.1.10"
port = 2222
groups = ["web"]
"#
        )
        .unwrap();

        let hosts = load_hosts(f.path()).unwrap();
        assert_eq!(hosts["web1"].port, Some(2222));
    }

    #[test]
    fn port_defaults_to_none() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"
[hosts.web1]
address = "192.168.1.10"
"#
        )
        .unwrap();

        let hosts = load_hosts(f.path()).unwrap();
        assert_eq!(hosts["web1"].port, None);
    }

    #[test]
    fn missing_hosts_file_returns_config_error() {
        let result = load_hosts(Path::new("/nonexistent/hosts.toml"));
        assert!(matches!(result, Err(Error::Config(_))));
    }
}
