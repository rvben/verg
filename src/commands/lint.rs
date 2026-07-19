use std::collections::BTreeSet;
use std::path::Path;

use crate::error::Error;
use crate::output::OutputConfig;

/// A single `$env.VAR` reference found in committed config.
#[derive(Debug, serde::Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct EnvRef {
    /// Where it was found, e.g. `group:plausible` or `host:web`.
    pub source: String,
    /// The property or variable key holding the reference.
    pub key: String,
    /// The referenced environment variable name (without the `$env.` prefix).
    pub env_var: String,
}

/// Recursively collect `$env.VAR` references from a TOML value. A reference is a
/// string whose entire value is `$env.<name>` (matching how vars resolve).
fn scan_value(source: &str, key: &str, value: &toml::Value, out: &mut Vec<EnvRef>) {
    match value {
        toml::Value::String(s) => {
            if let Some(var) = s.strip_prefix("$env.") {
                out.push(EnvRef {
                    source: source.to_string(),
                    key: key.to_string(),
                    env_var: var.to_string(),
                });
            }
        }
        toml::Value::Array(items) => {
            for item in items {
                scan_value(source, key, item, out);
            }
        }
        toml::Value::Table(table) => {
            for (k, v) in table {
                scan_value(source, &format!("{key}.{k}"), v, out);
            }
        }
        _ => {}
    }
}

/// Collect every `$env.` reference across group vars and host vars under
/// `base_dir`, sorted for deterministic output.
///
/// Only host and group vars resolve `$env.` (via `state::vars::toml_to_json`).
/// State resource properties are rendered as templates, so a literal `$env.FOO`
/// there is applied verbatim, not expanded - scanning them would be a false
/// positive. (Templates use the `env('FOO')` function for env access, a
/// separate form this audit does not cover.) Dynamic-inventory hosts are
/// included, matching what `diff`/`check`/`apply` resolve.
pub fn collect_env_refs(base_dir: &Path) -> Result<Vec<EnvRef>, Error> {
    use crate::inventory::static_hosts;
    let mut refs = Vec::new();

    let groups = crate::inventory::groups::load_groups(&base_dir.join("groups"))?;
    for (name, group) in &groups {
        for (key, value) in &group.vars {
            scan_value(&format!("group:{name}"), key, value, &mut refs);
        }
    }

    let hosts_path = base_dir.join("hosts.toml");
    if hosts_path.is_file() {
        let parsed = static_hosts::load_hosts(&hosts_path)?;
        let mut hosts = parsed.hosts;
        if let Some(cfg) = &parsed.inventory {
            // Mirror Inventory::load: a name defined both statically and
            // dynamically is an error, so lint fails exactly where a run would.
            for (name, def) in static_hosts::run_inventory_command(cfg, base_dir)? {
                if hosts.contains_key(&name) {
                    return Err(Error::Config(format!(
                        "host '{name}' is defined both in [hosts] and by the inventory command"
                    )));
                }
                hosts.insert(name, def);
            }
        }
        for (name, host) in &hosts {
            for (key, value) in &host.vars {
                scan_value(&format!("host:{name}"), key, value, &mut refs);
            }
        }
    }

    refs.sort();
    Ok(refs)
}

/// Audit the project for `$env.` secret references in committed config. These
/// tie the config to the ambient process environment, which is why such a
/// project cannot be committed and applied reproducibly without the same
/// environment. Read-only; exits 0 (informational).
pub fn run(base_dir: &Path, output: &OutputConfig) -> Result<i32, Error> {
    let refs = collect_env_refs(base_dir)?;
    let distinct: BTreeSet<&str> = refs.iter().map(|r| r.env_var.as_str()).collect();

    if output.json {
        let envelope = serde_json::json!({
            "env_refs": &refs,
            "total": refs.len(),
            "distinct_vars": distinct.iter().collect::<Vec<_>>(),
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| "{}".to_string())
        );
    } else if refs.is_empty() {
        println!("No $env references in committed config.");
    } else {
        println!(
            "{} $env reference(s) across {} variable(s). Committed config depends on the",
            refs.len(),
            distinct.len()
        );
        println!(
            "ambient environment; move secrets into verg/secrets.age so config is reproducible:"
        );
        for r in &refs {
            println!("  {}  {} -> $env.{}", r.source, r.key, r.env_var);
        }
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_value_finds_env_refs_in_nested_values() {
        let mut out = Vec::new();
        scan_value(
            "state:x.y",
            "url",
            &toml::Value::String("$env.API_URL".into()),
            &mut out,
        );
        // Not a ref: a plain string, and a string that merely contains $env.
        scan_value("s", "k", &toml::Value::String("plain".into()), &mut out);
        scan_value(
            "s",
            "k",
            &toml::Value::String("prefix $env.NOPE".into()),
            &mut out,
        );
        // Nested: array + table.
        let arr = toml::Value::Array(vec![toml::Value::String("$env.IN_ARRAY".into())]);
        scan_value("s", "list", &arr, &mut out);
        let mut tbl = toml::map::Map::new();
        tbl.insert("inner".into(), toml::Value::String("$env.IN_TABLE".into()));
        scan_value("s", "tbl", &toml::Value::Table(tbl), &mut out);

        let vars: Vec<&str> = out.iter().map(|r| r.env_var.as_str()).collect();
        assert_eq!(vars, vec!["API_URL", "IN_ARRAY", "IN_TABLE"]);
        assert_eq!(out[2].key, "tbl.inner");
    }

    #[test]
    fn collect_env_refs_scans_group_and_host_vars_only() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(dir.path().join("state")).unwrap();
        std::fs::create_dir_all(dir.path().join("groups")).unwrap();
        // A state resource property that is a literal "$env.APP_TOKEN": this is
        // NOT env-expanded (rendered as a template), so it must NOT be reported.
        std::fs::write(
            dir.path().join("state/app.toml"),
            "[resource.file.conf]\npath = \"/etc/app\"\ntoken = \"$env.APP_TOKEN\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("groups/web.toml"),
            "[vars]\nsecret = \"$env.WEB_SECRET\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("hosts.toml"),
            "[hosts.h1]\naddress = \"192.0.2.1\"\nuser = \"root\"\ngroups = [\"web\"]\n[hosts.h1.vars]\nkey = \"$env.H1_KEY\"\n",
        )
        .unwrap();

        let refs = collect_env_refs(dir.path()).unwrap();
        let vars: Vec<&str> = refs.iter().map(|r| r.env_var.as_str()).collect();
        // Only the group and host var refs resolve $env; the state prop does not.
        assert_eq!(vars, vec!["WEB_SECRET", "H1_KEY"]);
        assert!(
            !vars.contains(&"APP_TOKEN"),
            "state props are not $env-expanded"
        );
    }

    #[test]
    #[cfg(unix)]
    fn collect_env_refs_includes_dynamic_inventory_host_vars() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let script = dir.path().join("inv.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s' '{\"dyn1\":{\"address\":\"192.0.2.20\",\"vars\":{\"tok\":\"$env.DYN_TOKEN\"}}}'\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(
            dir.path().join("hosts.toml"),
            format!(
                "[inventory]\ncommand = [\"{}\"]\n",
                script.to_string_lossy()
            ),
        )
        .unwrap();

        let refs = collect_env_refs(dir.path()).unwrap();
        let vars: Vec<&str> = refs.iter().map(|r| r.env_var.as_str()).collect();
        assert_eq!(
            vars,
            vec!["DYN_TOKEN"],
            "dynamic-inventory host vars scanned"
        );
    }
}
