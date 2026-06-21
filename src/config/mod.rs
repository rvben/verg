//! Control-side configuration validation. Runs once before the per-host loop
//! so `diff`/`check`/`apply` reject typos locally rather than failing on the
//! remote agent (or silently doing the wrong thing).

use std::collections::HashMap;

use crate::error::Error;
use crate::resource_def::ResourceDef;
use crate::state::StateFile;

/// Validate prop names, required params, types, and enum constraints for a
/// custom or native provider resource instance. Shared by the custom-def and
/// provider-def branches of `validate_state_files`.
fn validate_param_props(
    policy: ConfigPolicy,
    fqn: &str,
    props: &toml::map::Map<String, toml::Value>,
    params: &HashMap<String, crate::resource_def::ParamSpec>,
) -> Result<(), Error> {
    let mut allowed: Vec<&str> = COMMON_FIELDS.to_vec();
    for param_name in params.keys() {
        allowed.push(param_name.as_str());
    }

    for (key, value) in props {
        if !allowed.contains(&key.as_str()) {
            report(
                policy,
                format!(
                    "{fqn}: unknown property '{key}'. Allowed: {}",
                    allowed.join(", ")
                ),
            )?;
        }
        check_special_key_type(policy, fqn, key, value)?;
    }

    for (param_name, param_spec) in params {
        if param_spec.required && !props.contains_key(param_name.as_str()) {
            report(
                policy,
                format!("{fqn}: missing required param '{param_name}'"),
            )?;
        }
    }

    for (param_name, param_spec) in params {
        if let Some(value) = props.get(param_name.as_str()) {
            let type_ok = matches!(
                (param_spec.param_type.as_str(), value),
                ("string", toml::Value::String(_))
                    | ("integer", toml::Value::Integer(_))
                    | ("float", toml::Value::Float(_))
                    | ("boolean", toml::Value::Boolean(_))
            );
            if !type_ok {
                report(
                    policy,
                    format!(
                        "{fqn}: param '{param_name}' must be a {} value",
                        param_spec.param_type
                    ),
                )?;
                continue;
            }
            if param_spec.param_type == "string"
                && let Some(enum_values) = &param_spec.enum_values
                && let toml::Value::String(s) = value
                && !enum_values.contains(s)
            {
                report(
                    policy,
                    format!(
                        "{fqn}: param '{param_name}' value '{s}' is not in allowed values: {}",
                        enum_values.join(", ")
                    ),
                )?;
            }
        }
    }
    Ok(())
}

/// Validate resource types, prop names, and special-key types across all state
/// files. Strict mode errors on the first violation; lax mode warns and continues.
/// Custom resource definitions in `custom_defs` and native provider definitions
/// in `provider_defs` extend the set of accepted types and are validated against
/// their param schemas.
pub fn validate_state_files(
    files: &[StateFile],
    policy: ConfigPolicy,
    custom_defs: &HashMap<String, ResourceDef>,
    provider_defs: &HashMap<String, crate::provider_def::ProviderDef>,
) -> Result<(), Error> {
    for sf in files {
        for decl in sf.resources()? {
            let fqn = format!("{}.{}", decl.resource_type, decl.name);

            let is_builtin = known_resource_types().contains(&decl.resource_type.as_str());
            let custom_def = custom_defs.get(&decl.resource_type);
            let provider_def = provider_defs.get(&decl.resource_type);

            if !is_builtin && custom_def.is_none() && provider_def.is_none() {
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

            if is_builtin {
                // Built-in types keep their exact existing code path.
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
            } else if let Some(def) = custom_def {
                validate_param_props(policy, &fqn, &decl.props, &def.params)?;
            } else {
                let def = provider_def.expect("provider_def is Some here");
                validate_param_props(policy, &fqn, &decl.props, &def.params)?;
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
        "sensitive" | "handler" | "template" if !value.is_bool() => {
            report(policy, format!("{fqn}: '{key}' must be a boolean"))?;
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

const ALLOWED_TOP_LEVEL_KEYS: &[&str] = &["targets", "resource"];

/// Reject unknown top-level keys in a state file (e.g. a typo'd `targets`,
/// which would otherwise silently apply the file to every host).
pub fn validate_state_file_toml(
    raw: &str,
    source: &str,
    policy: ConfigPolicy,
) -> Result<(), Error> {
    let table: toml::Table =
        toml::from_str(raw).map_err(|e| Error::Parse(format!("failed to parse {source}: {e}")))?;
    for key in table.keys() {
        if !ALLOWED_TOP_LEVEL_KEYS.contains(&key.as_str()) {
            report(
                policy,
                format!(
                    "{source}: unknown top-level key '{key}'. Allowed: {}",
                    ALLOWED_TOP_LEVEL_KEYS.join(", ")
                ),
            )?;
        }
    }
    Ok(())
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
        "git",
        "pkg",
        "file",
        "hostname",
        "mount",
        "service",
        "sysctl",
        "timezone",
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
    "after",
    "notify",
    "when",
    "handler",
    "template",
    "register",
    "vars",
    "sensitive",
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
        "git" => &["url", "path", "ref", "depth", "state"],
        "pkg" => &["name", "names", "state"],
        "file" => &["path", "content", "source", "mode", "owner"],
        "hostname" => &["hostname"],
        "service" => &["name", "state", "enabled"],
        "timezone" => &["timezone"],
        "docker_compose" => &["project_dir", "compose_file", "env_file", "state", "pull"],
        "mount" => &[
            "device", "path", "fstype", "options", "dump", "pass", "state",
        ],
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
    use std::collections::HashMap;

    use super::*;
    use crate::resource_def::{ParamSpec, ResourceDef};
    use crate::state::StateFile;

    fn parse(s: &str) -> StateFile {
        toml::from_str(s).unwrap()
    }

    /// Build a `lineinfile` ResourceDef for use in custom-resource tests.
    /// Params:
    ///   path   - string, required
    ///   line   - string, required
    ///   state  - string, default "present", enum [present, absent]
    fn lineinfile_def() -> ResourceDef {
        let mut params = HashMap::new();
        params.insert(
            "path".to_string(),
            ParamSpec {
                param_type: "string".to_string(),
                required: true,
                default: None,
                enum_values: None,
            },
        );
        params.insert(
            "line".to_string(),
            ParamSpec {
                param_type: "string".to_string(),
                required: true,
                default: None,
                enum_values: None,
            },
        );
        params.insert(
            "state".to_string(),
            ParamSpec {
                param_type: "string".to_string(),
                required: false,
                default: Some(toml::Value::String("present".to_string())),
                enum_values: Some(vec!["present".to_string(), "absent".to_string()]),
            },
        );
        ResourceDef {
            description: "Insert or remove a line in a file".to_string(),
            params,
            check: "grep -qF -- \"$VERG_PARAM_line\" \"$VERG_PARAM_path\"".to_string(),
            apply: "printf '%s\\n' \"$VERG_PARAM_line\" >> \"$VERG_PARAM_path\"".to_string(),
        }
    }

    fn lineinfile_defs() -> HashMap<String, ResourceDef> {
        let mut m = HashMap::new();
        m.insert("lineinfile".to_string(), lineinfile_def());
        m
    }

    #[test]
    fn custom_valid_instance_passes_strict() {
        let f = parse(
            "[resource.lineinfile.hosts_entry]\npath = \"/etc/hosts\"\nline = \"127.0.0.2 foo\"\nstate = \"present\"\n",
        );
        let defs = lineinfile_defs();
        validate_state_files(&[f], ConfigPolicy::strict(), &defs, &HashMap::new())
            .expect("valid custom resource should pass");
    }

    #[test]
    fn custom_missing_required_param_errors_in_strict() {
        // `path` is required but omitted.
        let f = parse("[resource.lineinfile.hosts_entry]\nline = \"127.0.0.2 foo\"\n");
        let defs = lineinfile_defs();
        let err =
            validate_state_files(&[f], ConfigPolicy::strict(), &defs, &HashMap::new()).unwrap_err();
        assert!(
            err.to_string().contains("path"),
            "error should mention missing param 'path', got: {err}"
        );
    }

    #[test]
    fn custom_unknown_param_key_errors_in_strict() {
        // `typo_key` is not in the def.
        let f = parse(
            "[resource.lineinfile.hosts_entry]\npath = \"/etc/hosts\"\nline = \"127.0.0.2 foo\"\ntypo_key = \"bad\"\n",
        );
        let defs = lineinfile_defs();
        let err =
            validate_state_files(&[f], ConfigPolicy::strict(), &defs, &HashMap::new()).unwrap_err();
        assert!(
            err.to_string().contains("typo_key"),
            "error should mention unknown key 'typo_key', got: {err}"
        );
    }

    #[test]
    fn custom_enum_value_outside_enum_errors_in_strict() {
        // `state = "gone"` is not in enum [present, absent].
        let f = parse(
            "[resource.lineinfile.hosts_entry]\npath = \"/etc/hosts\"\nline = \"127.0.0.2 foo\"\nstate = \"gone\"\n",
        );
        let defs = lineinfile_defs();
        let err =
            validate_state_files(&[f], ConfigPolicy::strict(), &defs, &HashMap::new()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("gone") && msg.contains("state"),
            "error should name the bad enum value and the param, got: {msg}"
        );
    }

    #[test]
    fn custom_wrong_typed_param_errors_in_strict() {
        // `path` is declared string but given an integer.
        let f = parse("[resource.lineinfile.hosts_entry]\npath = 42\nline = \"127.0.0.2 foo\"\n");
        let defs = lineinfile_defs();
        let err =
            validate_state_files(&[f], ConfigPolicy::strict(), &defs, &HashMap::new()).unwrap_err();
        assert!(
            err.to_string().contains("path"),
            "error should mention wrong-typed param 'path', got: {err}"
        );
    }

    #[test]
    fn entirely_unknown_type_still_errors_with_custom_defs() {
        // `frobble` is neither a built-in nor in the defs map.
        let f = parse("[resource.frobble.x]\nname = \"x\"\n");
        let defs = lineinfile_defs();
        let err =
            validate_state_files(&[f], ConfigPolicy::strict(), &defs, &HashMap::new()).unwrap_err();
        assert!(
            err.to_string().contains("frobble"),
            "error should mention unknown type 'frobble', got: {err}"
        );
    }

    #[test]
    fn custom_lax_policy_downgrades_errors_to_warnings() {
        // Missing required `path`, unknown key `typo_key`, bad enum `state=gone` -- all warnings in lax.
        let f = parse(
            "[resource.lineinfile.hosts_entry]\nline = \"127.0.0.2 foo\"\ntypo_key = \"bad\"\nstate = \"gone\"\n",
        );
        let defs = lineinfile_defs();
        assert!(
            validate_state_files(&[f], ConfigPolicy::lax(), &defs, &HashMap::new()).is_ok(),
            "lax mode should downgrade all custom validation errors to warnings"
        );
    }

    #[test]
    fn unknown_resource_type_is_rejected() {
        let f = parse("[resource.serrvice.nginx]\nname = \"nginx\"\n");
        let err = validate_state_files(
            &[f],
            ConfigPolicy::strict(),
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("serrvice"), "got: {err}");
    }

    #[test]
    fn misspelled_prop_is_rejected() {
        let f = parse("[resource.file.conf]\npath = \"/etc/x\"\nmod = \"0644\"\n");
        let err = validate_state_files(
            &[f],
            ConfigPolicy::strict(),
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("mod"), "got: {err}");
        assert!(err.to_string().contains("file.conf"), "got: {err}");
    }

    #[test]
    fn wrong_resource_property_is_rejected() {
        // `source` is a file-only build input; on pkg it would be silently
        // ignored, so it must be rejected.
        let f = parse("[resource.pkg.nginx]\nname = \"nginx\"\nsource = \"files/x\"\n");
        let err = validate_state_files(
            &[f],
            ConfigPolicy::strict(),
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("source"), "got: {err}");
    }

    #[test]
    fn wrong_typed_when_is_rejected() {
        let f = parse("[resource.service.nginx]\nname = \"nginx\"\nwhen = 1\n");
        let err = validate_state_files(
            &[f],
            ConfigPolicy::strict(),
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("when"), "got: {err}");
    }

    #[test]
    fn wrong_typed_after_item_is_rejected() {
        let f = parse("[resource.service.nginx]\nname = \"nginx\"\nafter = [42]\n");
        let err = validate_state_files(
            &[f],
            ConfigPolicy::strict(),
            &HashMap::new(),
            &HashMap::new(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("after"), "got: {err}");
    }

    #[test]
    fn lax_mode_allows_unknown_prop() {
        let f = parse("[resource.file.conf]\npath = \"/etc/x\"\nmod = \"0644\"\n");
        assert!(
            validate_state_files(&[f], ConfigPolicy::lax(), &HashMap::new(), &HashMap::new())
                .is_ok()
        );
    }

    #[test]
    fn valid_config_passes_strict() {
        let f = parse(
            "[resource.file.conf]\npath = \"/etc/x\"\nmode = \"0644\"\nwhen = \"group.web\"\nafter = [\"pkg.nginx\"]\n",
        );
        assert!(
            validate_state_files(
                &[f],
                ConfigPolicy::strict(),
                &HashMap::new(),
                &HashMap::new()
            )
            .is_ok()
        );
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
            "git",
            "pkg",
            "file",
            "hostname",
            "mount",
            "service",
            "sysctl",
            "timezone",
            "cmd",
            "cron",
            "user",
        ] {
            assert!(
                known_resource_types().contains(&t),
                "missing known resource type: {t}"
            );
        }
        assert_eq!(known_resource_types().len(), 15);
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

    #[test]
    fn typoed_targets_key_is_rejected() {
        let raw = "targetss = [\"web\"]\n[resource.pkg.nginx]\nname = \"nginx\"\n";
        let err = validate_state_file_toml(raw, "web.toml", ConfigPolicy::strict()).unwrap_err();
        assert!(err.to_string().contains("targetss"), "got: {err}");
        assert!(err.to_string().contains("web.toml"), "got: {err}");
    }

    #[test]
    fn valid_top_level_keys_pass() {
        let raw = "targets = [\"web\"]\n[resource.pkg.nginx]\nname = \"nginx\"\n";
        assert!(validate_state_file_toml(raw, "web.toml", ConfigPolicy::strict()).is_ok());
    }

    #[test]
    fn typoed_targets_warns_in_lax() {
        let raw = "targetss = [\"web\"]\n";
        assert!(validate_state_file_toml(raw, "web.toml", ConfigPolicy::lax()).is_ok());
    }

    #[test]
    fn provider_type_is_accepted_with_valid_params() {
        use crate::provider_def::ProviderDef;
        let f = parse(
            r#"
[resource.dns_record.www]
zone = "example.com"
"#,
        );
        let mut providers = HashMap::new();
        let mut params = HashMap::new();
        params.insert(
            "zone".to_string(),
            crate::resource_def::ParamSpec {
                param_type: "string".into(),
                required: true,
                default: None,
                enum_values: None,
            },
        );
        providers.insert(
            "dns_record".to_string(),
            ProviderDef {
                description: String::new(),
                interpreter: vec!["python3".into()],
                source: "x".into(),
                params,
            },
        );
        assert!(
            validate_state_files(&[f], ConfigPolicy::strict(), &HashMap::new(), &providers).is_ok()
        );
    }

    #[test]
    fn provider_missing_required_param_is_error() {
        use crate::provider_def::ProviderDef;
        let f = parse(
            r#"
[resource.dns_record.www]
"#,
        );
        let mut providers = HashMap::new();
        let mut params = HashMap::new();
        params.insert(
            "zone".to_string(),
            crate::resource_def::ParamSpec {
                param_type: "string".into(),
                required: true,
                default: None,
                enum_values: None,
            },
        );
        providers.insert(
            "dns_record".to_string(),
            ProviderDef {
                description: String::new(),
                interpreter: vec!["python3".into()],
                source: "x".into(),
                params,
            },
        );
        let err = validate_state_files(&[f], ConfigPolicy::strict(), &HashMap::new(), &providers)
            .unwrap_err();
        assert!(err.to_string().contains("zone"), "got: {err}");
    }
}
