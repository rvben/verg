use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::inventory::Host;
use crate::resource_def::ResourceDef;
use crate::resources::{REGISTER_SENTINEL, REGISTER_SENTINEL_END, ResolvedResource};
use crate::state::vars;
use crate::state::{ResourceDecl, StateFile};

fn protect_register_refs(input: &str) -> (String, Vec<String>) {
    let mut result = String::with_capacity(input.len());
    let mut names = Vec::new();
    let mut chars = input.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '{' && chars.peek() == Some(&'{') {
            chars.next(); // consume second {
            let mut inner = String::new();
            let mut found_close = false;
            while let Some(ch) = chars.next() {
                if ch == '}' && chars.peek() == Some(&'}') {
                    chars.next();
                    found_close = true;
                    break;
                }
                inner.push(ch);
            }
            if found_close {
                let trimmed = inner.trim();
                if let Some(reg_name) = trimmed.strip_prefix("register.") {
                    let reg_name = reg_name.trim().to_string();
                    result.push_str(REGISTER_SENTINEL);
                    result.push_str(&reg_name);
                    result.push_str(REGISTER_SENTINEL_END);
                    if !names.contains(&reg_name) {
                        names.push(reg_name);
                    }
                } else {
                    result.push_str("{{");
                    result.push_str(&inner);
                    result.push_str("}}");
                }
            } else {
                result.push_str("{{");
                result.push_str(&inner);
            }
        } else {
            result.push(c);
        }
    }

    (result, names)
}

fn restore_register_refs(input: &str) -> String {
    let mut result = input.to_string();
    while let Some(start) = result.find(REGISTER_SENTINEL) {
        let after_prefix = start + REGISTER_SENTINEL.len();
        if let Some(end) = result[after_prefix..].find(REGISTER_SENTINEL_END) {
            let name = result[after_prefix..after_prefix + end].to_string();
            let sentinel = format!("{REGISTER_SENTINEL}{name}{REGISTER_SENTINEL_END}");
            let replacement = format!("{{{{ register.{name} }}}}");
            result = result.replacen(&sentinel, &replacement, 1);
        } else {
            break;
        }
    }
    result
}

/// Render a file read from disk, optionally applying template interpolation.
/// Returns the rendered content and any register ref names found.
fn render_file(
    jinja: &minijinja::Environment,
    path: &Path,
    host_vars: &HashMap<String, toml::Value>,
    inventory_ctx: &serde_json::Value,
    is_template: bool,
    kind: &str,
    resource_fqn: &str,
) -> Result<(String, Vec<String>), Error> {
    let content = std::fs::read_to_string(path).map_err(|e| {
        Error::Config(format!(
            "failed to read {kind} file {}: {e}",
            path.display()
        ))
    })?;
    if is_template {
        let (protected, names) = protect_register_refs(&content);
        let rendered = vars::render_with_globals(jinja, &protected, host_vars, inventory_ctx)
            .map_err(|e| {
                Error::Config(format!(
                    "{resource_fqn}: template error in {kind} {}: {e}",
                    path.display()
                ))
            })?;
        Ok((restore_register_refs(&rendered), names))
    } else {
        Ok((content, Vec::new()))
    }
}

/// Extract resource fields from a declaration, interpolating string properties.
/// Returns the built resource and a list of register ref names found in its props.
fn build_resource(
    jinja: &minijinja::Environment,
    decl: &ResourceDecl,
    host: &Host,
    base_dir: &Path,
    inventory_ctx: &serde_json::Value,
) -> Result<(ResolvedResource, Vec<String>), Error> {
    let mut props = HashMap::new();
    let mut after = Vec::new();
    let mut notify = Vec::new();
    let mut when = None;
    let mut handler = false;
    let mut register = None;
    let mut sensitive = false;
    let mut register_refs = Vec::new();

    for (key, value) in &decl.props {
        if key == "when" {
            if let toml::Value::String(s) = value {
                when = Some(s.clone());
            }
        } else if key == "after" {
            if let toml::Value::Array(arr) = value {
                for item in arr {
                    if let toml::Value::String(s) = item {
                        after.push(s.clone());
                    }
                }
            }
        } else if key == "notify" {
            match value {
                toml::Value::String(s) => notify.push(s.clone()),
                toml::Value::Array(arr) => {
                    for item in arr {
                        if let toml::Value::String(s) = item {
                            notify.push(s.clone());
                        }
                    }
                }
                _ => {}
            }
        } else if key == "handler" {
            if let toml::Value::Boolean(b) = value {
                handler = *b;
            }
        } else if key == "sensitive" {
            if let toml::Value::Boolean(b) = value {
                sensitive = *b;
            }
        } else if key == "register" {
            if let toml::Value::String(s) = value {
                register = Some(s.clone());
            }
        } else {
            let interpolated = match value {
                toml::Value::String(s) => {
                    let (protected, names) = protect_register_refs(s);
                    register_refs.extend(names);
                    let rendered =
                        vars::render_with_globals(jinja, &protected, &host.vars, inventory_ctx)?;
                    toml::Value::String(restore_register_refs(&rendered))
                }
                other => other.clone(),
            };
            props.insert(key.clone(), interpolated);
        }
    }

    if let Some(toml::Value::Table(var_overrides)) = props.remove("vars") {
        for (k, v) in var_overrides {
            let as_string = match &v {
                toml::Value::String(s) => Some(s.clone()),
                toml::Value::Integer(i) => Some(i.to_string()),
                toml::Value::Float(f) => Some(f.to_string()),
                toml::Value::Boolean(b) => Some(b.to_string()),
                _ => None, // tables/arrays/datetimes are not valid scalar vars
            };
            if let Some(s) = as_string {
                let interpolated = vars::render_with_globals(jinja, &s, &host.vars, inventory_ctx)?;
                props.entry(k).or_insert(toml::Value::String(interpolated));
            }
        }
    }

    let is_template = props
        .remove("template")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let fqn = format!("{}.{}", decl.resource_type, decl.name);

    // Resolve `source` files on the control machine and inline as `content`
    if let Some(toml::Value::String(source_path)) = props.remove("source") {
        let (content, names) = render_file(
            jinja,
            &base_dir.join(&source_path),
            &host.vars,
            inventory_ctx,
            is_template,
            "source",
            &fqn,
        )?;
        register_refs.extend(names);
        props.insert("content".into(), toml::Value::String(content));
    }

    // Resolve `compose_file` for docker_compose resources
    if let Some(toml::Value::String(compose_path)) = props.remove("compose_file") {
        let (content, names) = render_file(
            jinja,
            &base_dir.join(&compose_path),
            &host.vars,
            inventory_ctx,
            is_template,
            "compose",
            &fqn,
        )?;
        register_refs.extend(names);
        props.insert("content".into(), toml::Value::String(content));
    }

    // Resolve `env_file` for docker_compose resources
    if let Some(toml::Value::String(env_path)) = props.remove("env_file") {
        let (content, names) = render_file(
            jinja,
            &base_dir.join(&env_path),
            &host.vars,
            inventory_ctx,
            is_template,
            "env",
            &fqn,
        )?;
        register_refs.extend(names);
        props.insert("env_content".into(), toml::Value::String(content));
    }

    // Deduplicate register refs
    register_refs.sort();
    register_refs.dedup();

    let resource = ResolvedResource {
        resource_type: decl.resource_type.clone(),
        name: decl.name.clone(),
        props,
        after,
        notify,
        when,
        handler,
        register,
        sensitive,
    };

    Ok((resource, register_refs))
}

/// Validate register wiring across the resources that built successfully.
///
/// A duplicate register name is a genuine authoring error that makes the whole
/// bundle ambiguous, so it aborts (returns `Err`). An unsatisfiable *reference*
/// (unknown register, or a missing `after` dependency) is scoped to the single
/// referring resource - it is returned as `(index, message)` so the caller can
/// fail just that resource and still diff the rest. A reference can be unknown
/// because its provider failed to build, so treating it per-resource keeps one
/// broken resource from taking down the host.
fn validate_registers(
    resources: &[ResolvedResource],
    register_refs_per_resource: &[Vec<String>],
) -> Result<Vec<(usize, String)>, Error> {
    // Validate register names are unique (fatal - the bundle is ambiguous otherwise).
    let mut register_names: HashMap<String, String> = HashMap::new();
    for r in resources {
        if let Some(ref reg_name) = r.register {
            if let Some(existing_fqn) = register_names.get(reg_name) {
                return Err(Error::Config(format!(
                    "duplicate register name '{reg_name}': used by both {existing_fqn} and {}",
                    r.fqn()
                )));
            }
            register_names.insert(reg_name.clone(), r.fqn());
        }
    }

    // Validate register references resolve and declare their `after` dependency.
    // Collect at most one failure per resource; the resource is then dropped.
    let mut ref_failures = Vec::new();
    for (i, (r, ref_names)) in resources.iter().zip(register_refs_per_resource).enumerate() {
        for ref_name in ref_names {
            match register_names.get(ref_name) {
                None => {
                    ref_failures.push((
                        i,
                        format!("{}: references unknown register '{ref_name}'", r.fqn()),
                    ));
                    break;
                }
                Some(reg_fqn) if !r.after.contains(reg_fqn) => {
                    ref_failures.push((
                        i,
                        format!(
                            "{}: uses register '{ref_name}' but does not declare after = [\"{reg_fqn}\"]",
                            r.fqn()
                        ),
                    ));
                    break;
                }
                Some(_) => {}
            }
        }
    }

    Ok(ref_failures)
}

/// Extract fact.* and group.* string vars for `when` conditional evaluation.
fn extract_facts(host_vars: &HashMap<String, toml::Value>) -> HashMap<String, String> {
    let mut facts = HashMap::new();
    for (k, v) in host_vars {
        if (k.starts_with("fact.") || k.starts_with("group."))
            && let toml::Value::String(s) = v
        {
            facts.insert(k.clone(), s.clone());
        }
    }
    facts
}

/// Returns the subset of `all_defs` whose type key appears in at least one resource.
/// The bundle ships only the defs the agent actually needs.
pub fn referenced_defs(
    resources: &[ResolvedResource],
    all_defs: &HashMap<String, ResourceDef>,
) -> HashMap<String, ResourceDef> {
    let used_types: std::collections::HashSet<&str> =
        resources.iter().map(|r| r.resource_type.as_str()).collect();
    all_defs
        .iter()
        .filter(|(k, _)| used_types.contains(k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Returns the subset of `all` whose type key appears in at least one resource.
pub fn referenced_provider_defs(
    resources: &[ResolvedResource],
    all: &HashMap<String, crate::provider_def::ProviderDef>,
) -> HashMap<String, crate::provider_def::ProviderDef> {
    let used_types: std::collections::HashSet<&str> =
        resources.iter().map(|r| r.resource_type.as_str()).collect();
    all.iter()
        .filter(|(k, _)| used_types.contains(k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// A resource that could not be built on the control machine (a template/secret
/// render error, or an unsatisfiable register reference). Reported as a failed
/// resource so the rest of the host still diffs, rather than aborting the whole
/// host on the first failure. Carries the resource's real type and name so the
/// diff/apply report shows the configured resource (e.g. `file.bad`), not a
/// synthetic identity.
#[derive(Debug, Clone)]
pub struct BuildFailure {
    pub resource_type: String,
    pub name: String,
    /// The error's `failure_kind()` (e.g. "config"/"resource"), carried so
    /// exit-code classification can treat a build failure by its real kind while
    /// the resource keeps its true `resource_type` for display.
    pub kind: &'static str,
    pub error: String,
}

impl BuildFailure {
    pub fn fqn(&self) -> String {
        format!("{}.{}", self.resource_type, self.name)
    }
}

/// A resource that could build, but is dropped because a resource it depends on
/// (via `after`) failed to build. Reported as Skipped ("dependency failed"),
/// mirroring how the agent skips dependents of a resource that fails at apply
/// time. It must not be transmitted: the agent's DAG rejects an `after` edge to
/// a resource absent from the bundle and would abort the whole host.
#[derive(Debug, Clone)]
pub struct BuildSkip {
    pub resource_type: String,
    pub name: String,
    pub reason: String,
}

impl BuildSkip {
    pub fn fqn(&self) -> String {
        format!("{}.{}", self.resource_type, self.name)
    }
}

/// The result of building a host bundle: the resources that built successfully
/// (ready to transmit), the per-resource build failures, and any resources
/// skipped because a dependency failed to build.
#[derive(Debug, Clone)]
pub struct BuildOutcome {
    pub bundle: Bundle,
    pub failures: Vec<BuildFailure>,
    pub skipped: Vec<BuildSkip>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bundle {
    pub host: String,
    pub resources: Vec<ResolvedResource>,
    #[serde(default)]
    pub facts: HashMap<String, String>,
    #[serde(default)]
    pub resource_defs: HashMap<String, ResourceDef>,
    #[serde(default)]
    pub provider_defs: HashMap<String, crate::provider_def::ProviderDef>,
}

impl Bundle {
    /// Build a bundle for a specific host.
    /// `base_dir` is the verg project directory (used to resolve `source` file paths).
    /// `inventory_ctx` exposes `inventory.hosts` and `inventory.groups` to templates.
    ///
    /// NOTE: this leaves `resource_defs` and `provider_defs` EMPTY. A caller that
    /// uses custom resource types MUST populate `resource_defs` before transmitting
    /// the bundle to the agent, e.g.
    /// `bundle.resource_defs = referenced_defs(&bundle.resources, &all_defs);`.
    /// A caller that uses native provider types MUST likewise set
    /// `bundle.provider_defs = referenced_provider_defs(&bundle.resources, &all_provider_defs);`.
    /// Without these steps the agent rejects any custom or provider type as "unknown resource type".
    pub fn build(
        host: &Host,
        state_files: &[StateFile],
        base_dir: &Path,
        inventory_ctx: &serde_json::Value,
    ) -> Result<BuildOutcome, Error> {
        let jinja = vars::create_env();
        let mut resources = Vec::new();
        let mut register_refs_per_resource: Vec<Vec<String>> = Vec::new();
        let mut failures: Vec<BuildFailure> = Vec::new();

        for sf in state_files {
            if let Some(targets) = &sf.targets {
                let applies = targets
                    .iter()
                    .any(|t| host.groups.contains(t) || host.name == *t);
                if !applies {
                    continue;
                }
            }

            // A per-resource build failure (e.g. an absent secret referenced by
            // one resource) becomes a failed resource, not a whole-host abort, so
            // a read-only diff still covers every resource that does build.
            for decl in sf.resources()? {
                match build_resource(&jinja, &decl, host, base_dir, inventory_ctx) {
                    Ok((resource, reg_refs)) => {
                        resources.push(resource);
                        register_refs_per_resource.push(reg_refs);
                    }
                    Err(e) => failures.push(BuildFailure {
                        resource_type: decl.resource_type.clone(),
                        name: decl.name.clone(),
                        kind: e.failure_kind(),
                        error: e.to_string(),
                    }),
                }
            }
        }

        // Register wiring: duplicates abort; an unsatisfiable reference fails only
        // its own resource (dropped from the bundle) so the rest still diffs.
        let ref_failures = validate_registers(&resources, &register_refs_per_resource)?;
        if ref_failures.is_empty() {
            // Common path: keep all resources, no reallocation.
        } else {
            let bad: std::collections::HashSet<usize> =
                ref_failures.iter().map(|(i, _)| *i).collect();
            let mut kept = Vec::with_capacity(resources.len() - bad.len());
            for (i, r) in resources.into_iter().enumerate() {
                if bad.contains(&i) {
                    let msg = ref_failures
                        .iter()
                        .find(|(idx, _)| *idx == i)
                        .map(|(_, m)| m.clone())
                        .unwrap_or_default();
                    failures.push(BuildFailure {
                        resource_type: r.resource_type.clone(),
                        name: r.name.clone(),
                        kind: "config",
                        error: msg,
                    });
                } else {
                    kept.push(r);
                }
            }
            resources = kept;
        }

        // Cascade: a dropped resource leaves any dependent (`after = ["dropped"]`)
        // pointing at a resource absent from the bundle, which the agent's DAG
        // rejects - aborting the whole host and defeating the partial diff. Drop
        // those dependents too, as Skipped ("dependency failed"), transitively,
        // mirroring how the agent skips dependents of a resource that fails at
        // apply time.
        let mut failed_fqns: std::collections::HashSet<String> =
            failures.iter().map(|f| f.fqn()).collect();
        let mut skipped: Vec<BuildSkip> = Vec::new();
        if !failed_fqns.is_empty() {
            loop {
                let mut moved = false;
                let mut kept = Vec::with_capacity(resources.len());
                for r in std::mem::take(&mut resources) {
                    match r.after.iter().find(|dep| failed_fqns.contains(*dep)) {
                        Some(dep) => {
                            skipped.push(BuildSkip {
                                resource_type: r.resource_type.clone(),
                                name: r.name.clone(),
                                reason: format!("dependency '{dep}' failed to build"),
                            });
                            failed_fqns.insert(r.fqn());
                            moved = true;
                        }
                        None => kept.push(r),
                    }
                }
                resources = kept;
                if !moved {
                    break;
                }
            }

            // A surviving resource may still `notify` a dropped resource (a handler
            // that failed to build is absent from the bundle). The agent, not
            // finding the FQN among handlers, would misread it as a shorthand
            // notify (e.g. `restart: <fqn>`) and emit a bogus action. `notify` is
            // weaker than `after` - the notifier does not depend on the handler to
            // run - so strip the dangling entries rather than dropping the resource.
            for r in &mut resources {
                r.notify.retain(|target| !failed_fqns.contains(target));
            }
        }

        let facts = extract_facts(&host.vars);

        Ok(BuildOutcome {
            bundle: Bundle {
                host: host.name.clone(),
                resources,
                facts,
                resource_defs: HashMap::new(),
                provider_defs: HashMap::new(),
            },
            failures,
            skipped,
        })
    }

    pub fn to_toml(&self) -> Result<String, Error> {
        toml::to_string_pretty(self)
            .map_err(|e| Error::Other(format!("failed to serialize bundle: {e}")))
    }

    pub fn from_toml(input: &str) -> Result<Self, Error> {
        toml::from_str(input).map_err(|e| Error::Parse(format!("failed to parse bundle: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_host() -> Host {
        Host {
            name: "web1".into(),
            address: "192.0.2.10".into(),
            user: "root".into(),
            port: None,
            groups: vec!["web".into(), "prod".into()],
            vars: {
                let mut v = HashMap::new();
                v.insert("http_port".into(), toml::Value::Integer(80));
                v.insert(
                    "document_root".into(),
                    toml::Value::String("/var/www".into()),
                );
                v
            },
        }
    }

    fn parse_state(toml_str: &str) -> StateFile {
        toml::from_str(toml_str).unwrap()
    }

    #[test]
    fn bundle_includes_matching_state_files() {
        let host = test_host();
        let files = vec![
            parse_state(
                r#"
[resource.pkg.curl]
name = "curl"
state = "present"
"#,
            ),
            parse_state(
                r#"
targets = ["web"]

[resource.pkg.nginx]
name = "nginx"
state = "present"
"#,
            ),
            parse_state(
                r#"
targets = ["db"]

[resource.pkg.postgres]
name = "postgresql"
state = "present"
"#,
            ),
        ];

        let bundle = Bundle::build(&host, &files, Path::new("/tmp"), &serde_json::Value::Null)
            .unwrap()
            .bundle;
        assert_eq!(bundle.resources.len(), 2);
    }

    #[test]
    fn bundle_interpolates_variables() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
targets = ["web"]

[resource.file.conf]
path = "/etc/nginx/nginx.conf"
content = "listen {{ http_port }}"
"#,
        )];

        let bundle = Bundle::build(&host, &files, Path::new("/tmp"), &serde_json::Value::Null)
            .unwrap()
            .bundle;
        assert_eq!(
            bundle.resources[0].props["content"],
            toml::Value::String("listen 80".into())
        );
    }

    #[test]
    fn bundle_extracts_after_dependencies() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.service.nginx]
name = "nginx"
state = "running"
after = ["pkg.nginx", "file.conf"]
"#,
        )];

        let bundle = Bundle::build(&host, &files, Path::new("/tmp"), &serde_json::Value::Null)
            .unwrap()
            .bundle;
        assert_eq!(bundle.resources[0].after, vec!["pkg.nginx", "file.conf"]);
        assert!(!bundle.resources[0].props.contains_key("after"));
    }

    #[test]
    fn bundle_roundtrip_toml() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.pkg.nginx]
name = "nginx"
state = "present"
"#,
        )];

        let bundle = Bundle::build(&host, &files, Path::new("/tmp"), &serde_json::Value::Null)
            .unwrap()
            .bundle;
        let serialized = bundle.to_toml().unwrap();
        let deserialized = Bundle::from_toml(&serialized).unwrap();
        assert_eq!(deserialized.host, "web1");
        assert_eq!(deserialized.resources.len(), 1);
        assert_eq!(deserialized.resources[0].fqn(), "pkg.nginx");
    }

    #[test]
    fn bundle_resolves_source_to_content() {
        let host = test_host();
        let dir = tempfile::TempDir::new().unwrap();
        let files_dir = dir.path().join("files");
        std::fs::create_dir(&files_dir).unwrap();
        std::fs::write(files_dir.join("test.conf"), "server_name web1;").unwrap();

        let files = vec![parse_state(
            r#"
[resource.file.conf]
path = "/etc/test.conf"
source = "files/test.conf"
"#,
        )];

        let bundle = Bundle::build(&host, &files, dir.path(), &serde_json::Value::Null)
            .unwrap()
            .bundle;
        assert_eq!(
            bundle.resources[0].props["content"],
            toml::Value::String("server_name web1;".into())
        );
        assert!(!bundle.resources[0].props.contains_key("source"));
    }

    #[test]
    fn undefined_variable_is_a_per_resource_failure() {
        // A resource whose template references an undefined variable fails to
        // build, but as a per-resource failure (kind "config") - not a whole-host
        // abort - so a read-only diff still covers every other resource.
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.file.conf]
content = "{{ undefined_var }}"

[resource.pkg.curl]
name = "curl"
state = "present"
"#,
        )];

        let outcome =
            Bundle::build(&host, &files, Path::new("/tmp"), &serde_json::Value::Null).unwrap();
        // The good resource still built.
        assert_eq!(outcome.bundle.resources.len(), 1);
        assert_eq!(outcome.bundle.resources[0].fqn(), "pkg.curl");
        // The broken one is reported as a failed resource, keeping its real
        // identity (type + name) so the report names the configured resource.
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(outcome.failures[0].resource_type, "file");
        assert_eq!(outcome.failures[0].name, "conf");
        assert_eq!(outcome.failures[0].fqn(), "file.conf");
    }

    #[test]
    fn dependents_of_a_failed_build_are_skipped_not_transmitted() {
        // A resource whose `after` names a resource that failed to build must be
        // dropped from the bundle (Skipped), transitively - otherwise the
        // transmitted bundle has an `after` edge to an absent resource and the
        // agent's DAG aborts the whole host. An unrelated resource still builds.
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.file.secret]
content = "{{ undefined_var }}"

[resource.pkg.app]
name = "app"
after = ["file.secret"]

[resource.pkg.plugin]
name = "plugin"
after = ["pkg.app"]

[resource.pkg.unrelated]
name = "unrelated"
"#,
        )];

        let outcome =
            Bundle::build(&host, &files, Path::new("/tmp"), &serde_json::Value::Null).unwrap();

        // Only the unrelated resource is transmitted.
        assert_eq!(outcome.bundle.resources.len(), 1);
        assert_eq!(outcome.bundle.resources[0].fqn(), "pkg.unrelated");
        // The root cause is a failure.
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(outcome.failures[0].fqn(), "file.secret");
        // Both the direct dependent and its transitive dependent are skipped.
        let skipped: std::collections::HashSet<String> =
            outcome.skipped.iter().map(|s| s.fqn()).collect();
        assert_eq!(
            skipped,
            ["pkg.app".to_string(), "pkg.plugin".to_string()]
                .into_iter()
                .collect(),
            "direct and transitive dependents must both be skipped"
        );
        // No transmitted resource has a dangling `after` edge.
        let present: std::collections::HashSet<String> =
            outcome.bundle.resources.iter().map(|r| r.fqn()).collect();
        for r in &outcome.bundle.resources {
            for dep in &r.after {
                assert!(present.contains(dep), "dangling after edge to {dep}");
            }
        }
    }

    #[test]
    fn notify_to_a_failed_handler_is_stripped() {
        // A handler that fails to build is dropped from the bundle. A surviving
        // resource that notifies it must have that notify entry stripped - else
        // the agent misreads the absent FQN as a shorthand notify and emits a
        // bogus action. The notifier still builds (notify is not a hard dep).
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.file.reload_marker]
handler = true
path = "/tmp/marker"
content = "{{ undefined_var }}"

[resource.file.conf]
path = "/etc/app.conf"
content = "static"
notify = ["file.reload_marker"]
"#,
        )];

        let outcome =
            Bundle::build(&host, &files, Path::new("/tmp"), &serde_json::Value::Null).unwrap();

        // The notifier still built.
        assert_eq!(outcome.bundle.resources.len(), 1);
        let conf = &outcome.bundle.resources[0];
        assert_eq!(conf.fqn(), "file.conf");
        // Its dangling notify was removed.
        assert!(
            conf.notify.is_empty(),
            "notify to the failed handler must be stripped, got: {:?}",
            conf.notify
        );
        // The handler is reported as a build failure.
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(outcome.failures[0].fqn(), "file.reload_marker");
    }

    #[test]
    fn bundle_renders_template_source_file() {
        let host = test_host();
        let dir = tempfile::TempDir::new().unwrap();
        let files_dir = dir.path().join("files");
        std::fs::create_dir(&files_dir).unwrap();
        std::fs::write(
            files_dir.join("test.conf.j2"),
            "listen {{ http_port }}\nroot {{ document_root }}",
        )
        .unwrap();

        let files = vec![parse_state(
            r#"
[resource.file.conf]
path = "/etc/test.conf"
source = "files/test.conf.j2"
template = true
"#,
        )];

        let bundle = Bundle::build(&host, &files, dir.path(), &serde_json::Value::Null)
            .unwrap()
            .bundle;
        assert_eq!(
            bundle.resources[0].props["content"],
            toml::Value::String("listen 80\nroot /var/www".into())
        );
    }

    #[test]
    fn bundle_does_not_render_source_without_template_flag() {
        let host = test_host();
        let dir = tempfile::TempDir::new().unwrap();
        let files_dir = dir.path().join("files");
        std::fs::create_dir(&files_dir).unwrap();
        std::fs::write(files_dir.join("raw.conf"), "{{ not_rendered }}").unwrap();

        let files = vec![parse_state(
            r#"
[resource.file.raw]
path = "/etc/raw.conf"
source = "files/raw.conf"
"#,
        )];

        let bundle = Bundle::build(&host, &files, dir.path(), &serde_json::Value::Null)
            .unwrap()
            .bundle;
        assert_eq!(
            bundle.resources[0].props["content"],
            toml::Value::String("{{ not_rendered }}".into())
        );
    }

    #[test]
    fn bundle_parses_sensitive_flag() {
        let host = test_host();
        let files = vec![parse_state(
            "[resource.cmd.secret]\ncommand = \"true\"\ncreates = \"/x\"\nsensitive = true\n",
        )];
        let bundle = Bundle::build(&host, &files, Path::new("/tmp"), &serde_json::Value::Null)
            .unwrap()
            .bundle;
        assert!(bundle.resources[0].sensitive);
    }

    #[test]
    fn bundle_extracts_handler_flag() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.cmd.nginx-reload]
command = "nginx -t && systemctl reload nginx"
handler = true
unless = "true"
"#,
        )];

        let bundle = Bundle::build(&host, &files, Path::new("/tmp"), &serde_json::Value::Null)
            .unwrap()
            .bundle;
        assert!(bundle.resources[0].handler);
        assert!(!bundle.resources[0].props.contains_key("handler"));
    }

    #[test]
    fn bundle_passes_through_register_references() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.cmd.get-ip]
command = "hostname -I"
register = "host_ip"

[resource.file.conf]
path = "/etc/app.conf"
content = "ip={{ register.host_ip }}"
after = ["cmd.get-ip"]
"#,
        )];

        let bundle = Bundle::build(&host, &files, Path::new("/tmp"), &serde_json::Value::Null)
            .unwrap()
            .bundle;
        let content = bundle.resources.iter().find(|r| r.name == "conf").unwrap();
        let val = content.props["content"].as_str().unwrap();
        assert!(
            val.contains("register.host_ip"),
            "register ref should survive: {val}"
        );
    }

    #[test]
    fn register_ref_without_dependency_fails_only_that_resource() {
        // Using a register without declaring the `after` dependency is an
        // authoring error, but it is scoped to the referring resource: that
        // resource fails, while the register provider (and any unrelated resource)
        // still builds.
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.cmd.get-ip]
command = "hostname -I"
register = "host_ip"

[resource.file.conf]
path = "/etc/app.conf"
content = "ip={{ register.host_ip }}"
"#,
        )];

        let outcome =
            Bundle::build(&host, &files, Path::new("/tmp"), &serde_json::Value::Null).unwrap();
        assert_eq!(outcome.bundle.resources.len(), 1);
        assert_eq!(outcome.bundle.resources[0].fqn(), "cmd.get-ip");
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(outcome.failures[0].fqn(), "file.conf");
        assert!(
            outcome.failures[0].error.contains("after"),
            "got: {}",
            outcome.failures[0].error
        );
    }

    #[test]
    fn unknown_register_ref_fails_only_that_resource() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.file.conf]
path = "/etc/app.conf"
content = "ip={{ register.nonexistent }}"
"#,
        )];

        let outcome =
            Bundle::build(&host, &files, Path::new("/tmp"), &serde_json::Value::Null).unwrap();
        assert!(outcome.bundle.resources.is_empty());
        assert_eq!(outcome.failures.len(), 1);
        assert_eq!(outcome.failures[0].fqn(), "file.conf");
        assert!(
            outcome.failures[0].error.contains("unknown register"),
            "got: {}",
            outcome.failures[0].error
        );
    }

    #[test]
    fn bundle_errors_on_duplicate_register_names() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.cmd.a]
command = "echo a"
register = "result"

[resource.cmd.b]
command = "echo b"
register = "result"
"#,
        )];

        let result = Bundle::build(&host, &files, Path::new("/tmp"), &serde_json::Value::Null);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("result"));
    }

    #[test]
    fn bundle_extracts_register_field() {
        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.cmd.get-ip]
command = "hostname -I"
register = "host_ip"
"#,
        )];

        let bundle = Bundle::build(&host, &files, Path::new("/tmp"), &serde_json::Value::Null)
            .unwrap()
            .bundle;
        assert_eq!(bundle.resources[0].register, Some("host_ip".into()));
        assert!(!bundle.resources[0].props.contains_key("register"));
    }

    #[test]
    fn bundle_renders_template_env_file() {
        let host = test_host();
        let dir = tempfile::TempDir::new().unwrap();
        let files_dir = dir.path().join("files");
        std::fs::create_dir(&files_dir).unwrap();
        std::fs::write(
            files_dir.join("compose.yml"),
            "version: '3'\nservices:\n  web:\n    image: nginx",
        )
        .unwrap();
        std::fs::write(
            files_dir.join("app.env"),
            "PORT={{ http_port }}\nROOT={{ document_root }}",
        )
        .unwrap();

        let files = vec![parse_state(
            r#"
[resource.docker_compose.app]
project_dir = "/opt/app"
compose_file = "files/compose.yml"
env_file = "files/app.env"
template = true
"#,
        )];

        let bundle = Bundle::build(&host, &files, dir.path(), &serde_json::Value::Null)
            .unwrap()
            .bundle;
        let env_content = bundle.resources[0].props["env_content"].as_str().unwrap();
        assert_eq!(env_content, "PORT=80\nROOT=/var/www");
    }

    #[test]
    fn inline_vars_coerces_scalar() {
        // The vars block inserts each entry as a resource PROP (after the main
        // string-prop interpolation pass), so assert the scalar became a string
        // prop instead of being silently dropped. It does not feed `content`.
        let host = test_host();
        let files = vec![parse_state(
            "[resource.file.conf]\npath = \"/etc/x\"\n[resource.file.conf.vars]\np = 8080\n",
        )];
        let bundle = Bundle::build(&host, &files, Path::new("/tmp"), &serde_json::Value::Null)
            .unwrap()
            .bundle;
        assert_eq!(
            bundle.resources[0].props.get("p"),
            Some(&toml::Value::String("8080".into())),
            "scalar inline var should be coerced to a string prop, not dropped"
        );
    }

    #[test]
    fn referenced_defs_returns_only_used_types() {
        use crate::resource_def::ResourceDef;

        let lineinfile_def = ResourceDef {
            description: "manages a line in a file".into(),
            params: HashMap::new(),
            check: "true".into(),
            apply: "true".into(),
        };
        let other_def = ResourceDef {
            description: "other resource".into(),
            params: HashMap::new(),
            check: "true".into(),
            apply: "true".into(),
        };

        let mut all_defs = HashMap::new();
        all_defs.insert("lineinfile".into(), lineinfile_def.clone());
        all_defs.insert("other".into(), other_def.clone());

        let resources = vec![
            ResolvedResource {
                resource_type: "lineinfile".into(),
                name: "hosts-entry".into(),
                props: HashMap::new(),
                after: vec![],
                notify: vec![],
                when: None,
                handler: false,
                register: None,
                sensitive: false,
            },
            ResolvedResource {
                resource_type: "lineinfile".into(),
                name: "motd-entry".into(),
                props: HashMap::new(),
                after: vec![],
                notify: vec![],
                when: None,
                handler: false,
                register: None,
                sensitive: false,
            },
        ];

        let result = referenced_defs(&resources, &all_defs);

        assert_eq!(result.len(), 1, "only lineinfile should be in the result");
        assert!(
            result.contains_key("lineinfile"),
            "lineinfile must be present"
        );
        assert!(!result.contains_key("other"), "other must not be present");
        assert_eq!(result["lineinfile"], lineinfile_def);
    }

    #[test]
    fn bundle_with_resource_defs_roundtrips_toml() {
        use crate::resource_def::ResourceDef;

        let host = test_host();
        let files = vec![parse_state(
            r#"
[resource.pkg.nginx]
name = "nginx"
state = "present"
"#,
        )];

        let mut bundle = Bundle::build(&host, &files, Path::new("/tmp"), &serde_json::Value::Null)
            .unwrap()
            .bundle;

        let custom_def = ResourceDef {
            description: "a custom resource type".into(),
            params: HashMap::new(),
            check: "test -f /opt/myapp/bin/myapp".into(),
            apply: "install-myapp".into(),
        };
        bundle
            .resource_defs
            .insert("myapp".into(), custom_def.clone());

        let serialized = bundle.to_toml().unwrap();
        let deserialized = Bundle::from_toml(&serialized).unwrap();

        assert_eq!(deserialized.resource_defs.len(), 1);
        assert_eq!(deserialized.resource_defs["myapp"], custom_def);
    }

    #[test]
    fn bundle_with_provider_defs_roundtrips_toml() {
        use crate::provider_def::ProviderDef;
        let mut bundle = Bundle {
            host: "web1".into(),
            resources: vec![],
            facts: HashMap::new(),
            resource_defs: HashMap::new(),
            provider_defs: HashMap::new(),
        };
        // Gnarly source: multi-line, embedded quotes, backslashes, and a triple
        // quote, to prove the TOML round-trip preserves arbitrary script text.
        let gnarly = "#!/usr/bin/env python3\nimport json,sys\nx = \"\"\"a\\nb\"\"\"\nprint('{\"status\":\"ok\"}')\n";
        bundle.provider_defs.insert(
            "dns_record".into(),
            ProviderDef {
                description: "DNS".into(),
                interpreter: vec!["python3".into()],
                source: gnarly.into(),
                params: HashMap::new(),
            },
        );
        let toml = bundle.to_toml().unwrap();
        let back = Bundle::from_toml(&toml).unwrap();
        assert_eq!(back.provider_defs.len(), 1);
        assert_eq!(
            back.provider_defs["dns_record"].source, gnarly,
            "embedded source must survive the TOML round-trip byte-for-byte"
        );
    }

    #[test]
    fn referenced_provider_defs_keeps_only_used() {
        use crate::provider_def::ProviderDef;
        let resources = vec![ResolvedResource {
            resource_type: "dns_record".into(),
            name: "www".into(),
            props: HashMap::new(),
            after: vec![],
            notify: vec![],
            when: None,
            handler: false,
            register: None,
            sensitive: false,
        }];
        let mut all = HashMap::new();
        for t in ["dns_record", "unused"] {
            all.insert(
                t.to_string(),
                ProviderDef {
                    description: String::new(),
                    interpreter: vec!["/bin/sh".into()],
                    source: "x".into(),
                    params: HashMap::new(),
                },
            );
        }
        let used = referenced_provider_defs(&resources, &all);
        assert_eq!(used.len(), 1);
        assert!(used.contains_key("dns_record"));
    }

    #[test]
    fn bundle_does_not_render_env_file_without_template_flag() {
        let host = test_host();
        let dir = tempfile::TempDir::new().unwrap();
        let files_dir = dir.path().join("files");
        std::fs::create_dir(&files_dir).unwrap();
        std::fs::write(
            files_dir.join("compose.yml"),
            "version: '3'\nservices:\n  web:\n    image: nginx",
        )
        .unwrap();
        std::fs::write(files_dir.join("app.env"), "PORT={{ not_rendered }}").unwrap();

        let files = vec![parse_state(
            r#"
[resource.docker_compose.app]
project_dir = "/opt/app"
compose_file = "files/compose.yml"
env_file = "files/app.env"
"#,
        )];

        let bundle = Bundle::build(&host, &files, dir.path(), &serde_json::Value::Null)
            .unwrap()
            .bundle;
        let env_content = bundle.resources[0].props["env_content"].as_str().unwrap();
        assert_eq!(env_content, "PORT={{ not_rendered }}");
    }
}
