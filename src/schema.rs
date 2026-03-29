use serde_json::{Value, json};

pub fn run() {
    let schema = json!({
        "tool": "verg",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Desired-state infrastructure convergence engine",
        "common_properties": {
            "after": {"type": "array", "items": {"type": "string"}, "description": "Resources that must complete before this one (FQN format: type.name)"},
            "notify": {"type": "array", "items": {"type": "string"}, "description": "Targets to notify on change. FQN for handler resources, or shorthand: restart:svc, reload:svc, daemon-reload, docker-restart:/path, docker-up:/path"},
            "when": {"type": "string", "description": "Conditional expression (e.g. fact.arch == 'x86_64', group.docker, !group.monitoring)"},
            "handler": {"type": "boolean", "description": "If true, resource only executes when notified (guards are bypassed)", "default": false},
            "template": {"type": "boolean", "description": "If true, source/compose_file content is rendered through the Jinja2 template engine", "default": false},
        },
        "resource_types": resource_schemas(),
    });
    println!("{}", serde_json::to_string_pretty(&schema).unwrap());
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
            "description": "Manage system packages (apt, dnf, pacman — auto-detected)",
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
        "cmd": {
            "description": "Run a command (requires idempotency guard, or register)",
            "properties": {
                "command": {"type": "string", "description": "Shell command to execute"},
                "creates": {"type": "string", "description": "Skip if this path exists"},
                "unless": {"type": "string", "description": "Skip if this command succeeds"},
                "onlyif": {"type": "string", "description": "Only run if this command succeeds"},
                "stdin": {"type": "string", "description": "Data to pipe to the command's stdin. Treated as sensitive — never echoed in diffs or output. Supports template variables (e.g. {{ smb_password }})."},
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
    use super::*;

    #[test]
    fn schema_has_all_resource_types() {
        let schemas = resource_schemas();
        let obj = schemas.as_object().unwrap();
        assert!(obj.contains_key("pkg"));
        assert!(obj.contains_key("file"));
        assert!(obj.contains_key("service"));
        assert!(obj.contains_key("cmd"));
        assert!(obj.contains_key("user"));
        assert!(obj.contains_key("sysctl"));
        assert!(obj.contains_key("cron"));
    }

    #[test]
    fn schema_has_common_properties() {
        let schema = json!({
            "tool": "verg",
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
    fn pkg_schema_has_required_fields() {
        let schemas = resource_schemas();
        let pkg = &schemas["pkg"];
        assert!(pkg["properties"]["name"].is_object());
        assert!(pkg["properties"]["state"].is_object());
    }
}
