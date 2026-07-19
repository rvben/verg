use std::collections::HashMap;

use serde_json::{Value, json};

use crate::resource_def::ResourceDef;

pub fn run(
    custom_defs: &HashMap<String, ResourceDef>,
    provider_defs: &HashMap<String, crate::provider_def::ProviderDef>,
) {
    let schema = json!({
        "clispec": "0.2",
        "name": "verg",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Desired-state infrastructure convergence engine",
        "global_args": [
            {"name": "--output", "type": "string", "enum": ["auto", "text", "json"], "default": "auto", "description": "Output format"},
            {"name": "--quiet", "type": "boolean", "description": "Suppress per-resource lines; print only the final summary"},
            {"name": "--path", "type": "path", "description": "Path to verg project directory"},
            {"name": "--parallel", "type": "integer", "default": 10, "description": "Maximum parallel connections"},
            {"name": "--ssh-config", "type": "path", "description": "Path to SSH config file"},
            {"name": "--yes", "type": "boolean", "description": "Skip confirmation prompts for destructive operations"},
            {"name": "--lax-config", "type": "boolean", "description": "Downgrade config validation errors (unknown keys/types) to warnings"},
            {"name": "--host-key-checking", "type": "string", "enum": ["yes", "accept-new", "no"], "default": "yes", "description": "SSH host key checking policy"},
            {"name": "--ssh-known-hosts", "type": "path", "description": "Path to a known_hosts file"},
            {"name": "--skip-agent-checksum", "type": "boolean", "description": "Skip agent binary checksum verification (air-gapped or local builds)"},
            {"name": "--timeout", "type": "integer", "default": 600, "description": "Per-host timeout in seconds"}
        ],
        "commands": [
            {
                "name": "apply",
                "description": "Converge targets to desired state",
                "mutating": true,
                "args": [
                    {"name": "--targets", "type": "string", "required": false, "description": "Target pattern to match hosts; required unless --plan is given (which carries its targets)"},
                    {"name": "--plan", "type": "path", "description": "Apply a saved plan; refuses (exit 8) if the diff drifted since planning"}
                ],
                "output_fields": [
                    {"name": "items", "type": "array", "description": "Per-host results; each item is an object with host (string), resources (array), and summary (object with ok/changed/failed/skipped counts)"},
                    {"name": "total", "type": "integer", "description": "Number of hosts in the result"}
                ]
            },
            {
                "name": "diff",
                "description": "Show what would change without applying",
                "mutating": false,
                "args": [
                    {"name": "--targets", "type": "string", "required": false, "default": "all", "description": "Target pattern to match hosts (default: all)"},
                    {"name": "--limit", "type": "integer", "default": 100, "description": "Maximum number of hosts to return (pagination is per host, not per resource)"},
                    {"name": "--offset", "type": "integer", "default": 0, "description": "Number of hosts to skip"},
                    {"name": "--fields", "type": "string", "description": "Comma-separated list of fields to include"}
                ],
                "output_fields": [
                    {"name": "items", "type": "array", "description": "Per-host results (paginated); each item is an object with host (string), summary (object), and resources (array)"},
                    {"name": "total", "type": "integer", "description": "Total number of hosts (the pagination unit)"},
                    {"name": "limit", "type": "integer"},
                    {"name": "offset", "type": "integer"}
                ]
            },
            {
                "name": "check",
                "description": "Verify targets match desired state",
                "mutating": false,
                "args": [
                    {"name": "--targets", "type": "string", "required": false, "default": "all", "description": "Target pattern to match hosts (default: all)"}
                ],
                "output_fields": [
                    {"name": "items", "type": "array", "description": "Per-host results; each item is an object with host (string), resources (array), and summary (object with ok/changed/failed/skipped counts)"},
                    {"name": "total", "type": "integer", "description": "Number of hosts in the result"}
                ]
            },
            {
                "name": "plan",
                "description": "Compute a diff and save it as a reviewable plan for apply --plan",
                "mutating": false,
                "args": [
                    {"name": "--targets", "type": "string", "required": false, "default": "all", "description": "Target pattern to match hosts (default: all)"},
                    {"name": "--out", "type": "path", "required": true, "description": "File to write the plan to"}
                ],
                "output_fields": [
                    {"name": "plan", "type": "string", "description": "Path the plan was written to"},
                    {"name": "hosts", "type": "integer"},
                    {"name": "changed", "type": "integer"},
                    {"name": "failed", "type": "integer"}
                ]
            },
            {
                "name": "lint",
                "description": "Audit committed config for $env secret references (read-only)",
                "mutating": false,
                "args": [],
                "output_fields": [
                    {"name": "env_refs", "type": "array", "description": "Each item has source (e.g. state:pkg.docker, group:web, host:h1), key, and env_var"},
                    {"name": "total", "type": "integer", "description": "Number of $env references found"},
                    {"name": "distinct_vars", "type": "array", "description": "Sorted distinct environment variable names referenced"}
                ]
            },
            {
                "name": "schema",
                "description": "Print resource type schemas as JSON",
                "mutating": false,
                "args": [],
                "output_fields": []
            },
            {
                "name": "init",
                "description": "Scaffold a new verg project directory",
                "mutating": true,
                "args": [
                    {"name": "--force", "type": "boolean", "default": false, "description": "Overwrite existing scaffold files"}
                ],
                "output_fields": []
            },
            {
                "name": "completions",
                "description": "Generate shell completions",
                "mutating": false,
                "args": [
                    {"name": "shell", "type": "string", "required": true, "enum": ["bash", "fish", "zsh", "powershell", "elvish"]}
                ],
                "output_fields": []
            }
        ],
        "errors": [
            {"kind": "invalid_config", "exit_code": 5, "retryable": false, "description": "Configuration file is missing, malformed, or contains invalid values"},
            {"kind": "connection_error", "exit_code": 4, "retryable": true, "description": "SSH connection to a target host failed"},
            {"kind": "not_found", "exit_code": 6, "retryable": false, "description": "No hosts matched the given target pattern"},
            {"kind": "resource_error", "exit_code": 2, "retryable": false, "description": "One or more resources failed to converge"},
            {"kind": "internal_error", "exit_code": 7, "retryable": false, "description": "Unexpected internal error"},
            {"kind": "confirmation_required", "exit_code": 5, "retryable": false, "description": "Operation requires confirmation; pass --yes to proceed non-interactively"},
            {"kind": "conflict", "exit_code": 8, "retryable": false, "description": "State conflict that cannot be automatically resolved"}
        ],
        "exit_codes": [
            {"code": 0, "meaning": "success", "description": "Completed; one or more resources changed (for diff/check: drift was found)"},
            {"code": 1, "meaning": "nothing_changed", "description": "Completed; everything already matched desired state (no changes, no drift)"},
            {"code": 2, "meaning": "partial_failure", "description": "Some resources or hosts failed while others succeeded"},
            {"code": 3, "meaning": "total_failure", "description": "All resources failed with no successes (not a connection issue)"},
            {"code": 4, "meaning": "connection_error", "description": "Could not reach the target host(s) over SSH"},
            {"code": 5, "meaning": "invalid_config", "description": "Configuration missing, malformed, or invalid (also: confirmation required without --yes)"},
            {"code": 6, "meaning": "target_not_found", "description": "No hosts matched the target selector"},
            {"code": 7, "meaning": "internal_error", "description": "Unexpected internal error"},
            {"code": 8, "meaning": "conflict", "description": "State conflict that cannot be automatically resolved"},
            {"code": 130, "meaning": "interrupted", "description": "Interrupted by SIGINT (Ctrl-C)"}
        ],
        "common_properties": {
            "after": {"type": "array", "items": {"type": "string"}, "description": "Resources that must converge first"},
            "notify": {"type": "array", "items": {"type": "string"}, "description": "Handlers/actions to trigger on change"},
            "when": {"type": "string", "description": "Conditional expression (e.g. fact.os == 'Ubuntu')"},
            "handler": {"type": "boolean", "default": false, "description": "Run only when notified"},
            "template": {"type": "boolean", "default": false, "description": "Render source/content as a Jinja template"},
            "register": {"type": "string", "description": "Capture stdout under a name for downstream {{ register.NAME }}"},
            "vars": {"type": "object", "description": "Resource-scoped variable overrides"},
            "sensitive": {"type": "boolean", "default": false, "description": "Redact this resource's diff/from/to/output from output and the changelog"}
        },
        "resource_types": build_resource_types(custom_defs, provider_defs),
    });
    println!("{}", serde_json::to_string_pretty(&schema).unwrap());
}

fn build_resource_types(
    custom_defs: &HashMap<String, ResourceDef>,
    provider_defs: &HashMap<String, crate::provider_def::ProviderDef>,
) -> Value {
    let mut types = resource_schemas();
    let map = types
        .as_object_mut()
        .expect("resource_schemas returns an object");

    for (type_name, def) in custom_defs {
        let mut properties = serde_json::Map::new();
        for (param_name, param) in &def.params {
            let mut prop = serde_json::Map::new();
            prop.insert("type".to_string(), json!(param.param_type));
            prop.insert("required".to_string(), json!(param.required));
            if let Some(default) = &param.default {
                let default_json = toml_value_to_json(default);
                prop.insert("default".to_string(), default_json);
            }
            if let Some(enum_values) = &param.enum_values {
                prop.insert("enum".to_string(), json!(enum_values));
            }
            properties.insert(param_name.clone(), Value::Object(prop));
        }
        let entry = json!({
            "description": def.description,
            "custom": true,
            "properties": Value::Object(properties),
        });
        map.insert(type_name.clone(), entry);
    }

    for (type_name, def) in provider_defs {
        let mut properties = serde_json::Map::new();
        for (param_name, param) in &def.params {
            let mut prop = serde_json::Map::new();
            prop.insert("type".to_string(), json!(param.param_type));
            prop.insert("required".to_string(), json!(param.required));
            if let Some(default) = &param.default {
                let default_json = toml_value_to_json(default);
                prop.insert("default".to_string(), default_json);
            }
            if let Some(enum_values) = &param.enum_values {
                prop.insert("enum".to_string(), json!(enum_values));
            }
            properties.insert(param_name.clone(), Value::Object(prop));
        }
        let entry = json!({
            "description": def.description,
            "provider": true,
            "properties": Value::Object(properties),
        });
        map.insert(type_name.clone(), entry);
    }

    types
}

fn toml_value_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => json!(s),
        toml::Value::Integer(i) => json!(i),
        toml::Value::Float(f) => json!(f),
        toml::Value::Boolean(b) => json!(b),
        toml::Value::Array(arr) => Value::Array(arr.iter().map(toml_value_to_json).collect()),
        toml::Value::Table(tbl) => {
            let map: serde_json::Map<_, _> = tbl
                .iter()
                .map(|(k, v)| (k.clone(), toml_value_to_json(v)))
                .collect();
            Value::Object(map)
        }
        toml::Value::Datetime(dt) => json!(dt.to_string()),
    }
}

fn resource_schemas() -> Value {
    json!({
        "apt_repo": {
            "description": "Manage APT repositories with GPG keys",
            "properties": {
                "name": {"type": "string", "description": "Repository identifier (used for filenames)"},
                "url": {"type": "string", "description": "Base URL of the repository"},
                "gpg_key": {"type": "string", "description": "URL to the GPG signing key"},
                "suite": {"type": "string", "description": "Distribution suite (default: auto-detected)"},
                "component": {"type": "string", "description": "Repository component (default: 'stable')"},
                "arch": {"type": "string", "description": "Architecture (default: 'amd64')"},
                "state": {"type": "string", "enum": ["present", "absent"], "default": "present"},
            },
            "required": ["name", "url", "gpg_key"],
        },
        "directory": {
            "description": "Manage directories with ownership and permissions",
            "properties": {
                "path": {"type": "string", "description": "Directory path"},
                "owner": {"type": "string", "description": "Owner (username or UID)"},
                "group": {"type": "string", "description": "Group (groupname or GID)"},
                "mode": {"type": "string", "description": "Permissions (octal, e.g. '0755')"},
                "recurse": {"type": "boolean", "description": "Apply ownership recursively", "default": false},
                "state": {"type": "string", "enum": ["present", "absent"], "default": "present"},
            },
            "required": ["path"],
        },
        "download": {
            "description": "Download a file from a URL, optionally extract archives",
            "properties": {
                "url": {"type": "string", "description": "URL to download from"},
                "dest": {"type": "string", "description": "Destination path on target"},
                "mode": {"type": "string", "description": "File permissions (octal)"},
                "owner": {"type": "string", "description": "File owner"},
                "extract": {"type": "boolean", "description": "Extract archive (zip, tar.gz)", "default": false},
                "checksum": {"type": "string", "description": "SHA256 checksum to verify download"},
                "state": {"type": "string", "enum": ["present", "absent"], "default": "present"},
            },
            "required": ["url", "dest"],
        },
        "pkg": {
            "description": "Manage system packages (apt, dnf, pacman - auto-detected)",
            "properties": {
                "name": {"type": "string", "description": "Package name (single)"},
                "names": {"type": "array", "items": {"type": "string"}, "description": "Package names (multiple)"},
                "state": {"type": "string", "enum": ["present", "absent"], "default": "present"},
            },
            "required_one_of": ["name", "names"],
        },
        "file": {
            "description": "Manage files and directories",
            "properties": {
                "path": {"type": "string", "description": "Absolute path on target"},
                "content": {"type": "string", "description": "Desired file content (inline)"},
                "source": {"type": "string", "description": "Source file path (relative to verg dir)"},
                "mode": {"type": "string", "description": "File permissions (octal, e.g. '0644')"},
                "owner": {"type": "string", "description": "File owner"},
            },
            "required": ["path"],
        },
        "hostname": {
            "description": "Set the static system hostname",
            "properties": {
                "hostname": {"type": "string", "description": "Desired static hostname"},
            },
            "required": ["hostname"],
        },
        "mount": {
            "description": "Manage /etc/fstab entries and mount state",
            "properties": {
                "device": {"type": "string", "description": "Block device or remote share (e.g. /dev/sdb1, UUID=..., server:/export)"},
                "path": {"type": "string", "description": "Absolute mountpoint path (no whitespace; fstab \\040 encoding not supported in v1)"},
                "fstype": {"type": "string", "description": "Filesystem type (e.g. ext4, xfs, nfs, tmpfs)"},
                "options": {"type": "string", "description": "Mount options (default: 'defaults')"},
                "dump": {"type": "string", "description": "fstab dump field (default: '0')"},
                "pass": {"type": "string", "description": "fstab pass field for fsck order (default: '0')"},
                "state": {"type": "string", "enum": ["mounted", "absent"], "default": "mounted"},
            },
            "required": ["device", "path", "fstype"],
            "note": "Writes /etc/fstab atomically before mounting. mount/umount require root. Mountpoints containing whitespace are rejected; use bind mounts or rename the path.",
        },
        "service": {
            "description": "Manage systemd services",
            "properties": {
                "name": {"type": "string", "description": "Service name"},
                "state": {"type": "string", "enum": ["running", "stopped"], "default": "running"},
                "enabled": {"type": "boolean", "description": "Whether the service starts on boot"},
            },
            "required": ["name"],
        },
        "docker_compose": {
            "description": "Manage Docker Compose services",
            "properties": {
                "project_dir": {"type": "string", "description": "Directory on target for compose project"},
                "compose_file": {"type": "string", "description": "Path to compose file (relative to verg dir, resolved at build time)"},
                "env_file": {"type": "string", "description": "Path to .env file (relative to verg dir, resolved at build time)"},
                "state": {"type": "string", "enum": ["up", "down"], "default": "up"},
                "pull": {"type": "boolean", "description": "Pull images before starting", "default": true},
            },
            "required": ["project_dir"],
        },
        "sysctl": {
            "description": "Manage Linux kernel parameters",
            "properties": {
                "key": {"type": "string", "description": "Sysctl key (e.g. net.ipv4.ip_forward)"},
                "value": {"type": "string", "description": "Desired value"},
                "persist": {"type": "boolean", "description": "Write to /etc/sysctl.d/99-verg.conf for persistence across reboots", "default": false},
            },
            "required": ["key", "value"],
        },
        "timezone": {
            "description": "Set the system timezone (IANA tz database name)",
            "properties": {
                "timezone": {"type": "string", "description": "IANA timezone name (e.g. 'Europe/Amsterdam')"},
            },
            "required": ["timezone"],
        },
        "cmd": {
            "description": "Run a command (requires idempotency guard, or register)",
            "properties": {
                "command": {"type": "string", "description": "Shell command to execute"},
                "creates": {"type": "string", "description": "Skip if this path exists"},
                "unless": {"type": "string", "description": "Skip if this command succeeds"},
                "onlyif": {"type": "string", "description": "Only run if this command succeeds"},
                "stdin": {"type": "string", "description": "Data to pipe to the command's stdin. Treated as sensitive - never echoed in diffs or output. Supports template variables (e.g. {{ smb_password }})."},
                "register": {"type": "string", "description": "Capture stdout into a named register for use in downstream resources via {{ register.NAME }}"},
            },
            "required": ["command"],
            "required_one_of_guards": ["creates", "unless", "onlyif", "register"],
        },
        "user": {
            "description": "Manage system users",
            "properties": {
                "name": {"type": "string", "description": "Username"},
                "state": {"type": "string", "enum": ["present", "absent"], "default": "present"},
                "home": {"type": "string", "description": "Home directory path"},
                "shell": {"type": "string", "description": "Login shell"},
                "groups": {"type": "string", "description": "Supplementary groups (comma-separated)"},
            },
            "required": ["name"],
        },
        "git": {
            "description": "Clone a git repository and ensure it is checked out at the desired ref",
            "properties": {
                "url": {"type": "string", "description": "Repository URL to clone"},
                "path": {"type": "string", "description": "Local checkout directory on the target"},
                "ref": {"type": "string", "description": "Branch, tag, or SHA to check out (default: repository default branch)"},
                "depth": {"type": "string", "description": "Shallow clone depth passed to --depth (integer as string)"},
                "state": {"type": "string", "enum": ["present", "absent"], "default": "present"},
            },
            "required": ["url", "path"],
            "note": "SHA refs trigger a post-clone checkout rather than --branch. Combining depth with a SHA ref may fail if the SHA is not in the shallow history.",
        },
        "cron": {
            "description": "Manage cron jobs via /etc/cron.d/<name> files",
            "properties": {
                "name": {"type": "string", "description": "Cron file name (alphanumeric, hyphens, underscores only)"},
                "schedule": {"type": "string", "description": "Cron schedule expression (5 fields, single-job form)"},
                "command": {"type": "string", "description": "Command to run (single-job form)"},
                "user": {"type": "string", "description": "User to run the job as (default: root)"},
                "jobs": {"type": "array", "description": "Multiple jobs (multi-job form; mutually exclusive with schedule/command)", "items": {
                    "type": "object",
                    "properties": {
                        "schedule": {"type": "string"},
                        "command": {"type": "string"},
                        "user": {"type": "string"},
                    }
                }},
                "mailto": {"type": "string", "description": "MAILTO value (default: empty string to suppress mail)"},
                "env": {"type": "object", "description": "Additional environment variables to set in the cron file"},
                "state": {"type": "string", "enum": ["present", "absent"], "default": "present"},
            },
            "required": ["name"],
            "note": "Use single-job form (schedule + command) or multi-job form (jobs array), not both",
        },
    })
}

#[cfg(test)]
mod tests {
    use crate::resource_def::ParamSpec;

    use super::*;

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

    #[test]
    fn custom_resource_appears_in_schema_with_custom_marker() {
        let mut defs = HashMap::new();
        defs.insert("lineinfile".to_string(), lineinfile_def());

        let types = build_resource_types(&defs, &HashMap::new());
        let obj = types.as_object().unwrap();

        // Custom type is present.
        assert!(
            obj.contains_key("lineinfile"),
            "lineinfile must appear in resource_types"
        );

        let lif = &obj["lineinfile"];
        // Must carry the "custom": true marker.
        assert_eq!(lif["custom"], true, "custom type must have 'custom': true");
        // Description is forwarded.
        assert_eq!(
            lif["description"], "Insert or remove a line in a file",
            "description must be forwarded"
        );

        let props = lif["properties"].as_object().unwrap();

        // path: required, no default, no enum.
        let path = &props["path"];
        assert_eq!(path["type"], "string");
        assert_eq!(path["required"], true);
        assert!(path.get("default").is_none(), "path has no default");
        assert!(path.get("enum").is_none(), "path has no enum");

        // line: required, no default, no enum.
        let line = &props["line"];
        assert_eq!(line["type"], "string");
        assert_eq!(line["required"], true);

        // state: optional, default "present", enum [present, absent].
        let state = &props["state"];
        assert_eq!(state["type"], "string");
        assert_eq!(state["required"], false);
        assert_eq!(
            state["default"], "present",
            "state default must be 'present'"
        );
        let enum_vals: Vec<&str> = state["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(enum_vals, vec!["present", "absent"]);
    }

    #[test]
    fn builtin_types_have_no_custom_marker() {
        let defs = HashMap::new();
        let types = build_resource_types(&defs, &HashMap::new());
        let obj = types.as_object().unwrap();

        for builtin in &["pkg", "file", "service", "cmd", "user", "sysctl", "cron"] {
            let entry = &obj[*builtin];
            assert!(
                entry.get("custom").is_none(),
                "built-in type '{builtin}' must NOT have a 'custom' key"
            );
        }
    }

    #[test]
    fn empty_custom_defs_leaves_schema_unchanged() {
        let without_custom = build_resource_types(&HashMap::new(), &HashMap::new());
        let with_custom = {
            let defs = HashMap::new();
            build_resource_types(&defs, &HashMap::new())
        };
        assert_eq!(
            without_custom, with_custom,
            "empty custom_defs must not change schema output"
        );
        // And all builtins are present.
        let obj = without_custom.as_object().unwrap();
        for t in &["pkg", "file", "service", "cmd", "user", "sysctl", "cron"] {
            assert!(obj.contains_key(*t));
        }
    }

    #[test]
    fn schema_has_all_resource_types() {
        let schemas = resource_schemas();
        let obj = schemas.as_object().unwrap();
        // Every built-in type must have a schema entry. Keep this in sync with
        // config::known_resource_types().
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
            "hostname",
            "timezone",
            "mount",
            "git",
        ] {
            assert!(obj.contains_key(t), "schema missing resource type: {t}");
        }
        assert_eq!(obj.len(), 15, "schema resource_types count drifted");
    }

    #[test]
    fn schema_has_common_properties() {
        let schema = json!({
            "name": "verg",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Desired-state infrastructure convergence engine",
            "common_properties": {
                "after": {"type": "array", "items": {"type": "string"}},
                "notify": {"type": "array", "items": {"type": "string"}},
                "when": {"type": "string"},
                "handler": {"type": "boolean", "default": false},
                "template": {"type": "boolean", "default": false},
            },
            "resource_types": resource_schemas(),
        });
        let obj = schema.as_object().unwrap();
        assert!(obj.contains_key("common_properties"));
        let common = obj["common_properties"].as_object().unwrap();
        assert!(common.contains_key("after"));
        assert!(common.contains_key("notify"));
        assert!(common.contains_key("when"));
        assert!(common.contains_key("handler"));
        assert!(common.contains_key("template"));
    }

    #[test]
    fn real_schema_common_properties_includes_sensitive() {
        // Verify that the schema emitted by run() includes common_properties.sensitive
        // so agents can discover the attribute without reading source.
        let schema = json!({
            "clispec": "0.2",
            "name": "verg",
            "version": env!("CARGO_PKG_VERSION"),
            "description": "Desired-state infrastructure convergence engine",
            "global_args": [],
            "commands": [],
            "errors": [],
            "common_properties": {
                "after": {"type": "array", "items": {"type": "string"}, "description": "Resources that must converge first"},
                "notify": {"type": "array", "items": {"type": "string"}, "description": "Handlers/actions to trigger on change"},
                "when": {"type": "string", "description": "Conditional expression (e.g. fact.os == 'Ubuntu')"},
                "handler": {"type": "boolean", "default": false, "description": "Run only when notified"},
                "template": {"type": "boolean", "default": false, "description": "Render source/content as a Jinja template"},
                "register": {"type": "string", "description": "Capture stdout under a name for downstream {{ register.NAME }}"},
                "vars": {"type": "object", "description": "Resource-scoped variable overrides"},
                "sensitive": {"type": "boolean", "default": false, "description": "Redact this resource's diff/from/to/output from output and the changelog"}
            },
            "resource_types": resource_schemas(),
        });
        let obj = schema.as_object().unwrap();
        let common = obj["common_properties"].as_object().unwrap();
        assert!(
            common.contains_key("sensitive"),
            "common_properties must include 'sensitive'"
        );
        assert_eq!(common["sensitive"]["type"], "boolean");
        assert_eq!(common["sensitive"]["default"], false);
    }

    #[test]
    fn provider_types_appear_in_schema() {
        use crate::provider_def::ProviderDef;
        let mut providers = HashMap::new();
        providers.insert(
            "dns_record".to_string(),
            ProviderDef {
                description: "DNS".into(),
                interpreter: vec!["python3".into()],
                source: "x".into(),
                params: HashMap::new(),
            },
        );
        let value = build_resource_types(&HashMap::new(), &providers);
        let obj = value.as_object().expect("object");
        let entry = obj
            .get("dns_record")
            .and_then(|v| v.as_object())
            .expect("provider type must be in schema");
        assert_eq!(
            entry.get("provider").and_then(|v| v.as_bool()),
            Some(true),
            "provider entry must carry the \"provider\": true marker"
        );
        assert_eq!(
            entry.get("description").and_then(|v| v.as_str()),
            Some("DNS"),
            "provider description must be exposed in the schema"
        );
    }

    #[test]
    fn pkg_schema_has_required_fields() {
        let schemas = resource_schemas();
        let pkg = &schemas["pkg"];
        assert!(pkg["properties"]["name"].is_object());
        assert!(pkg["properties"]["state"].is_object());
    }

    #[test]
    fn schema_has_clispec_v0_2_fields() {
        let schema = json!({
            "clispec": "0.2",
            "name": "verg",
            "version": env!("CARGO_PKG_VERSION"),
            "commands": [],
        });
        let obj = schema.as_object().unwrap();
        assert_eq!(obj["clispec"], "0.2");
        assert_eq!(obj["name"], "verg");
        assert!(obj.contains_key("version"));
        assert!(obj.contains_key("commands"));
    }
}
