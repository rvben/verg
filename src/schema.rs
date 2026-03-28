use serde_json::{Value, json};

pub fn run() {
    let schema = json!({
        "tool": "verg",
        "version": env!("CARGO_PKG_VERSION"),
        "description": "Desired-state infrastructure convergence engine",
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
                "template": {"type": "string", "description": "Template file with {{ var }} interpolation"},
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
        "cmd": {
            "description": "Run a command (requires idempotency guard)",
            "properties": {
                "command": {"type": "string", "description": "Shell command to execute"},
                "creates": {"type": "string", "description": "Skip if this path exists"},
                "unless": {"type": "string", "description": "Skip if this command succeeds"},
                "onlyif": {"type": "string", "description": "Only run if this command succeeds"},
            },
            "required": ["command"],
            "required_one_of_guards": ["creates", "unless", "onlyif"],
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
    }

    #[test]
    fn pkg_schema_has_required_fields() {
        let schemas = resource_schemas();
        let pkg = &schemas["pkg"];
        assert!(pkg["properties"]["name"].is_object());
        assert!(pkg["properties"]["state"].is_object());
    }
}
