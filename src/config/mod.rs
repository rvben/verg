//! Control-side configuration validation. Runs once before the per-host loop
//! so `diff`/`check`/`apply` reject typos locally rather than failing on the
//! remote agent (or silently doing the wrong thing).

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
        "cmd" => &[
            "command", "creates", "unless", "onlyif", "stdin", "register",
        ],
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
    }
}
