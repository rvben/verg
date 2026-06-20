//! Control-side configuration validation. Runs once before the per-host loop
//! so `diff`/`check`/`apply` reject typos locally rather than failing on the
//! remote agent (or silently doing the wrong thing).

use crate::error::Error;
use crate::state::StateFile;

/// Validate resource types, prop names, and special-key types across all state
/// files. Strict mode errors on the first violation; lax mode warns and continues.
pub fn validate_state_files(files: &[StateFile], policy: ConfigPolicy) -> Result<(), Error> {
    for sf in files {
        for decl in sf.resources()? {
            let fqn = format!("{}.{}", decl.resource_type, decl.name);

            if !known_resource_types().contains(&decl.resource_type.as_str()) {
                report(
                    policy,
                    format!(
                        "{fqn}: unknown resource type '{}'. Known types: {}",
                        decl.resource_type,
                        known_resource_types().join(", ")
                    ),
                )?;
                // Unknown type has no field list; skip per-field checks.
                continue;
            }

            let allowed = allowed_fields(&decl.resource_type)
                .expect("known type must have an allowed-field list");

            for (key, value) in &decl.props {
                if !allowed.contains(&key.as_str()) {
                    report(
                        policy,
                        format!(
                            "{fqn}: unknown property '{key}'. Allowed: {}",
                            allowed.join(", ")
                        ),
                    )?;
                }
                check_special_key_type(policy, &fqn, key, value)?;
            }
        }
    }
    Ok(())
}

fn check_special_key_type(
    policy: ConfigPolicy,
    fqn: &str,
    key: &str,
    value: &toml::Value,
) -> Result<(), Error> {
    match key {
        "when" | "register" if !value.is_str() => {
            report(policy, format!("{fqn}: '{key}' must be a string"))?;
        }
        "when" | "register" => {}
        "after" => match value.as_array() {
            Some(arr) if arr.iter().all(|v| v.is_str()) => {}
            _ => report(
                policy,
                format!("{fqn}: 'after' must be an array of strings"),
            )?,
        },
        "notify" => {
            let ok = value.is_str()
                || value
                    .as_array()
                    .is_some_and(|arr| arr.iter().all(|v| v.is_str()));
            if !ok {
                report(
                    policy,
                    format!("{fqn}: 'notify' must be a string or an array of strings"),
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// In strict mode, return a Config error. In lax mode, warn to stderr and continue.
fn report(policy: ConfigPolicy, message: String) -> Result<(), Error> {
    if policy.strict {
        Err(Error::Config(message))
    } else {
        eprintln!("warning: {message}");
        Ok(())
    }
}

/// How strictly to treat unknown keys, unknown resource types, and wrong-typed
/// special keys. `strict` errors; `lax` downgrades to warnings on stderr.
#[derive(Debug, Clone, Copy)]
pub struct ConfigPolicy {
    pub strict: bool,
}

impl ConfigPolicy {
    pub fn strict() -> Self {
        ConfigPolicy { strict: true }
    }
    pub fn lax() -> Self {
        ConfigPolicy { strict: false }
    }
}

/// Resource types the agent's `execute_resource` dispatcher handles. Keep in
/// sync with `src/resources/mod.rs::execute_resource`; `known_types_match_dispatcher`
/// guards the count.
pub fn known_resource_types() -> &'static [&'static str] {
    &[
        "apt_repo",
        "directory",
        "docker_compose",
        "download",
        "pkg",
        "file",
        "service",
        "sysctl",
        "cmd",
        "cron",
        "user",
    ]
}

/// Keys valid on any resource: ordering, handler, templating, register, and
/// inline vars. Type-specific keys (`name`, `state`, `source`, `compose_file`,
/// `env_file`, ...) live ONLY in `specific_fields` so that, e.g., `source` on a
/// `pkg` or `env_file` on a `service` is correctly rejected as a wrong-resource
/// property instead of silently ignored.
const COMMON_FIELDS: &[&str] = &[
    "after", "notify", "when", "handler", "template", "register", "vars",
];

/// Resource-specific allowed props, mirroring `src/schema.rs`.
fn specific_fields(resource_type: &str) -> Option<&'static [&'static str]> {
    let fields: &'static [&'static str] = match resource_type {
        "apt_repo" => &[
            "name",
            "url",
            "gpg_key",
            "suite",
            "component",
            "arch",
            "state",
        ],
        "directory" => &["path", "owner", "group", "mode", "recurse", "state"],
        "download" => &[
            "url", "dest", "mode", "owner", "extract", "checksum", "state",
        ],
        "pkg" => &["name", "names", "state"],
        "file" => &["path", "content", "source", "mode", "owner"],
        "service" => &["name", "state", "enabled"],
        "docker_compose" => &["project_dir", "compose_file", "env_file", "state", "pull"],
        "sysctl" => &["key", "value", "persist"],
        "cmd" => &["command", "creates", "unless", "onlyif", "stdin"],
        "user" => &["name", "state", "home", "shell", "groups"],
        "cron" => &[
            "name", "schedule", "command", "user", "jobs", "mailto", "env", "state",
        ],
        _ => return None,
    };
    Some(fields)
}

/// Allowed field names for a resource type: common keys unioned with the
/// type's specific props. `None` if the type is unknown.
pub fn allowed_fields(resource_type: &str) -> Option<Vec<&'static str>> {
    let specific = specific_fields(resource_type)?;
    let mut all: Vec<&'static str> = COMMON_FIELDS.to_vec();
    for f in specific {
        if !all.contains(f) {
            all.push(*f);
        }
    }
    Some(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::StateFile;

    fn parse(s: &str) -> StateFile {
        toml::from_str(s).unwrap()
    }

    #[test]
    fn unknown_resource_type_is_rejected() {
        let f = parse("[resource.serrvice.nginx]\nname = \"nginx\"\n");
        let err = validate_state_files(&[f], ConfigPolicy::strict()).unwrap_err();
        assert!(err.to_string().contains("serrvice"), "got: {err}");
    }

    #[test]
    fn misspelled_prop_is_rejected() {
        let f = parse("[resource.file.conf]\npath = \"/etc/x\"\nmod = \"0644\"\n");
        let err = validate_state_files(&[f], ConfigPolicy::strict()).unwrap_err();
        assert!(err.to_string().contains("mod"), "got: {err}");
        assert!(err.to_string().contains("file.conf"), "got: {err}");
    }

    #[test]
    fn wrong_resource_property_is_rejected() {
        // `source` is a file-only build input; on pkg it would be silently
        // ignored, so it must be rejected.
        let f = parse("[resource.pkg.nginx]\nname = \"nginx\"\nsource = \"files/x\"\n");
        let err = validate_state_files(&[f], ConfigPolicy::strict()).unwrap_err();
        assert!(err.to_string().contains("source"), "got: {err}");
    }

    #[test]
    fn wrong_typed_when_is_rejected() {
        let f = parse("[resource.service.nginx]\nname = \"nginx\"\nwhen = 1\n");
        let err = validate_state_files(&[f], ConfigPolicy::strict()).unwrap_err();
        assert!(err.to_string().contains("when"), "got: {err}");
    }

    #[test]
    fn wrong_typed_after_item_is_rejected() {
        let f = parse("[resource.service.nginx]\nname = \"nginx\"\nafter = [42]\n");
        let err = validate_state_files(&[f], ConfigPolicy::strict()).unwrap_err();
        assert!(err.to_string().contains("after"), "got: {err}");
    }

    #[test]
    fn lax_mode_allows_unknown_prop() {
        let f = parse("[resource.file.conf]\npath = \"/etc/x\"\nmod = \"0644\"\n");
        assert!(validate_state_files(&[f], ConfigPolicy::lax()).is_ok());
    }

    #[test]
    fn valid_config_passes_strict() {
        let f = parse(
            "[resource.file.conf]\npath = \"/etc/x\"\nmode = \"0644\"\nwhen = \"group.web\"\nafter = [\"pkg.nginx\"]\n",
        );
        assert!(validate_state_files(&[f], ConfigPolicy::strict()).is_ok());
    }

    #[test]
    fn known_types_match_dispatcher() {
        // Every type the agent can execute must be a known type here, so the
        // control-side validator never rejects a type the agent supports.
        for t in [
            "apt_repo",
            "directory",
            "docker_compose",
            "download",
            "pkg",
            "file",
            "service",
            "sysctl",
            "cmd",
            "cron",
            "user",
        ] {
            assert!(
                known_resource_types().contains(&t),
                "missing known resource type: {t}"
            );
        }
        assert_eq!(known_resource_types().len(), 11);
    }

    #[test]
    fn allowed_fields_includes_common_and_specific() {
        let f = allowed_fields("file").unwrap();
        assert!(f.contains(&"path")); // resource-specific
        assert!(f.contains(&"content")); // resource-specific
        assert!(f.contains(&"after")); // common
        assert!(f.contains(&"when")); // common
        assert!(f.contains(&"template")); // build-time
        assert!(allowed_fields("nonsuch").is_none());

        // Type-specific keys must NOT bleed across types.
        let pkg = allowed_fields("pkg").unwrap();
        assert!(!pkg.contains(&"source")); // file-only build input
        assert!(!pkg.contains(&"compose_file")); // docker_compose-only
        // register is a common field, available everywhere including cmd.
        assert!(allowed_fields("cmd").unwrap().contains(&"register"));
    }
}
