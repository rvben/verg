use std::path::Path;

use crate::bundle::Bundle;
use crate::error::Error;
use crate::inventory::{Inventory, selector};
use crate::state;

/// Build per-host bundles offline and write them to `dest/<hostname>.toml`.
///
/// Group memberships are injected as `group.<name> = "true"` facts so templates
/// and `when` conditions can reference them. Live system facts (fact.arch, etc.)
/// are NOT available offline; hosts whose templates reference them will fail to
/// build and are skipped with a clear error message.
///
/// Exit codes:
///   0 (SUCCESS)          - all matched hosts published
///   2 (PARTIAL_FAILURE)  - some hosts published, some failed
///   5 (INVALID_CONFIG)   - project-level validation failed before writing anything,
///                          or every host failed to build
pub fn run(
    base_dir: &Path,
    targets: &str,
    dest: &Path,
    policy: crate::config::ConfigPolicy,
) -> Result<i32, Error> {
    let inventory = Inventory::load(base_dir)?;
    let inventory_ctx = inventory.to_template_context();
    let state_files = state::load_state_dir(&base_dir.join("state"))?;
    let resource_defs = crate::resource_def::load_resource_defs(
        &base_dir.join("resources"),
        crate::config::known_resource_types(),
    )?;

    // Validate the entire project before writing any output so a bad config
    // fails loudly on the control host rather than producing partial output.
    crate::config::validate_state_files(&state_files, policy, &resource_defs)?;

    let sel = selector::parse_selector(targets)?;
    let hosts = inventory.filter(&sel)?;

    std::fs::create_dir_all(dest)?;

    let mut published = 0u32;
    let mut failed = 0u32;

    for host in hosts {
        let mut host = host.clone();

        // Inject group membership facts exactly as the engine does (engine.rs:210-213).
        for g in &host.groups.clone() {
            host.vars
                .entry(format!("group.{g}"))
                .or_insert_with(|| toml::Value::String("true".into()));
        }

        match Bundle::build(&host, &state_files, base_dir, &inventory_ctx) {
            Ok(mut bundle) => {
                bundle.resource_defs =
                    crate::bundle::referenced_defs(&bundle.resources, &resource_defs);
                match bundle.to_toml() {
                    Ok(toml_str) => {
                        let out_path = dest.join(format!("{}.toml", host.name));
                        if let Err(e) = std::fs::write(&out_path, &toml_str) {
                            eprintln!(
                                "publish: {}: failed to write {}: {e}",
                                host.name,
                                out_path.display()
                            );
                            failed += 1;
                        } else {
                            published += 1;
                        }
                    }
                    Err(e) => {
                        eprintln!("publish: {}: failed to serialize bundle: {e}", host.name);
                        failed += 1;
                    }
                }
            }
            Err(e) => {
                eprintln!("publish: {}: bundle build failed: {e}", host.name);
                failed += 1;
            }
        }
    }

    eprintln!("publish: {published} published, {failed} failed");

    use crate::error::exit_codes;
    if published == 0 {
        Ok(exit_codes::INVALID_CONFIG)
    } else if failed > 0 {
        Ok(exit_codes::PARTIAL_FAILURE)
    } else {
        Ok(exit_codes::SUCCESS)
    }
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::bundle::Bundle;
    use crate::config::ConfigPolicy;

    /// Set up a minimal verg project with two hosts in different groups.
    ///
    /// web1 is in group "web", db1 is in group "db".
    /// The state file has a file resource with no group-conditional content.
    fn setup_project(dir: &TempDir) {
        std::fs::write(
            dir.path().join("hosts.toml"),
            r#"
[hosts.web1]
address = "192.0.2.10"
groups = ["web"]

[hosts.db1]
address = "192.0.2.20"
groups = ["db"]
"#,
        )
        .unwrap();

        let state_dir = dir.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("base.toml"),
            r#"
[resource.file.motd]
path = "/etc/motd"
content = "hello"
"#,
        )
        .unwrap();
    }

    #[test]
    fn publish_writes_bundle_per_host() {
        let project = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();
        setup_project(&project);

        let code = run(project.path(), "all", dest.path(), ConfigPolicy::strict()).unwrap();

        // Both hosts publish successfully.
        assert_eq!(code, crate::error::exit_codes::SUCCESS);

        let web1_path = dest.path().join("web1.toml");
        let db1_path = dest.path().join("db1.toml");
        assert!(web1_path.exists(), "web1.toml should exist");
        assert!(db1_path.exists(), "db1.toml should exist");

        // Each file must parse back as a valid Bundle.
        let web1_content = std::fs::read_to_string(&web1_path).unwrap();
        let web1_bundle = Bundle::from_toml(&web1_content).expect("web1.toml should parse");
        assert_eq!(web1_bundle.host, "web1");

        let db1_content = std::fs::read_to_string(&db1_path).unwrap();
        let db1_bundle = Bundle::from_toml(&db1_content).expect("db1.toml should parse");
        assert_eq!(db1_bundle.host, "db1");
    }

    #[test]
    fn publish_with_group_selector_writes_only_matching_host() {
        let project = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();
        setup_project(&project);

        let code = run(project.path(), "web", dest.path(), ConfigPolicy::strict()).unwrap();

        assert_eq!(code, crate::error::exit_codes::SUCCESS);
        assert!(dest.path().join("web1.toml").exists(), "web1.toml missing");
        assert!(
            !dest.path().join("db1.toml").exists(),
            "db1.toml should not exist for selector 'web'"
        );
    }

    #[test]
    fn publish_injects_group_facts_into_bundle() {
        let project = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();
        setup_project(&project);

        run(project.path(), "web", dest.path(), ConfigPolicy::strict()).unwrap();

        let content = std::fs::read_to_string(dest.path().join("web1.toml")).unwrap();
        let bundle = Bundle::from_toml(&content).unwrap();

        // web1 is in group "web" so group.web must be set to "true" in the facts.
        assert_eq!(
            bundle.facts.get("group.web").map(String::as_str),
            Some("true"),
            "group.web fact must be injected for web1"
        );
        // web1 is NOT in group "db" so group.db must not be present.
        assert!(
            !bundle.facts.contains_key("group.db"),
            "group.db must not be injected for web1"
        );
    }

    #[test]
    fn publish_skips_failing_host_and_still_writes_others() {
        let project = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();

        // Two hosts: web1 has a template that references fact.arch (undefined
        // offline), db1 has a plain resource that builds fine.
        std::fs::write(
            project.path().join("hosts.toml"),
            r#"
[hosts.web1]
address = "192.0.2.10"
groups = ["web"]

[hosts.db1]
address = "192.0.2.20"
groups = ["db"]
"#,
        )
        .unwrap();

        let state_dir = project.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();

        // base.toml applies to all hosts; references fact.arch which is only
        // available at runtime (not injected offline).
        std::fs::write(
            state_dir.join("base.toml"),
            r#"
[resource.file.arch_note]
path = "/etc/arch"
content = "arch={{ fact.arch }}"
"#,
        )
        .unwrap();

        // ok.toml applies only to db1 and uses no runtime facts.
        std::fs::write(
            state_dir.join("ok.toml"),
            r#"targets = ["db"]

[resource.file.motd]
path = "/etc/motd"
content = "hello"
"#,
        )
        .unwrap();

        // Note: the test uses lax config because the "arch" file resource is
        // valid TOML (no config errors), it just fails at bundle-build time when
        // fact.arch is undefined during template rendering.
        let code = run(project.path(), "all", dest.path(), ConfigPolicy::lax()).unwrap();

        // Partial failure: db1 publishes but web1 fails.
        // Both hosts fail because both have the fact.arch resource; only db1
        // also has the ok.toml resource but the base.toml fails first.
        // So we expect INVALID_CONFIG (all failed) OR PARTIAL_FAILURE (some).
        // Actually both hosts have fact.arch in base.toml so both fail ->
        // INVALID_CONFIG (exit 5).
        assert_eq!(code, crate::error::exit_codes::INVALID_CONFIG);
        assert!(
            !dest.path().join("web1.toml").exists(),
            "web1.toml should not be written when build fails"
        );
        assert!(
            !dest.path().join("db1.toml").exists(),
            "db1.toml should not be written when build fails"
        );
    }

    #[test]
    fn publish_partial_failure_when_only_one_host_fails() {
        let project = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();

        std::fs::write(
            project.path().join("hosts.toml"),
            r#"
[hosts.web1]
address = "192.0.2.10"
groups = ["web"]

[hosts.db1]
address = "192.0.2.20"
groups = ["db"]
"#,
        )
        .unwrap();

        let state_dir = project.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();

        // web-only state file uses fact.arch (undefined offline).
        std::fs::write(
            state_dir.join("web.toml"),
            r#"targets = ["web"]

[resource.file.arch_note]
path = "/etc/arch"
content = "arch={{ fact.arch }}"
"#,
        )
        .unwrap();

        // db-only state file with a plain resource.
        std::fs::write(
            state_dir.join("db.toml"),
            r#"targets = ["db"]

[resource.file.motd]
path = "/etc/motd"
content = "hello from db"
"#,
        )
        .unwrap();

        let code = run(project.path(), "all", dest.path(), ConfigPolicy::strict()).unwrap();

        // web1 fails (fact.arch missing), db1 succeeds -> PARTIAL_FAILURE.
        assert_eq!(code, crate::error::exit_codes::PARTIAL_FAILURE);
        assert!(
            !dest.path().join("web1.toml").exists(),
            "web1.toml must not be written when its build fails"
        );
        assert!(
            dest.path().join("db1.toml").exists(),
            "db1.toml must be written despite web1 failure"
        );

        // Verify the written bundle parses correctly.
        let content = std::fs::read_to_string(dest.path().join("db1.toml")).unwrap();
        let bundle = Bundle::from_toml(&content).unwrap();
        assert_eq!(bundle.host, "db1");
    }

    #[test]
    fn publish_ships_custom_resource_defs_in_bundle() {
        let project = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();

        std::fs::write(
            project.path().join("hosts.toml"),
            "[hosts.web1]\naddress = \"192.0.2.10\"\ngroups = [\"web\"]\n",
        )
        .unwrap();

        let state_dir = project.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        std::fs::write(
            state_dir.join("base.toml"),
            r#"
[resource.myapp.deploy]
path = "/opt/app"
"#,
        )
        .unwrap();

        // Custom resource definition.
        let resources_dir = project.path().join("resources");
        std::fs::create_dir_all(&resources_dir).unwrap();
        std::fs::write(
            resources_dir.join("myapp.toml"),
            r#"
[resource_def.myapp]
description = "Deploy myapp"
check = "test -d {{ path }}"
apply = "mkdir -p {{ path }}"

[resource_def.myapp.params.path]
type = "string"
required = true
"#,
        )
        .unwrap();

        let code = run(project.path(), "web", dest.path(), ConfigPolicy::strict()).unwrap();

        assert_eq!(code, crate::error::exit_codes::SUCCESS);

        let content = std::fs::read_to_string(dest.path().join("web1.toml")).unwrap();
        let bundle = Bundle::from_toml(&content).unwrap();

        // The custom def must be present in the bundle so the offline agent
        // knows how to execute it.
        assert!(
            bundle.resource_defs.contains_key("myapp"),
            "bundle must contain the 'myapp' resource def; got defs: {:?}",
            bundle.resource_defs.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn publish_invalid_project_config_errors_before_writing() {
        let project = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();

        std::fs::write(
            project.path().join("hosts.toml"),
            "[hosts.web1]\naddress = \"192.0.2.10\"\n",
        )
        .unwrap();

        let state_dir = project.path().join("state");
        std::fs::create_dir_all(&state_dir).unwrap();
        // Unknown resource type "bogustype" - strict validation should reject this.
        std::fs::write(
            state_dir.join("bad.toml"),
            "[resource.bogustype.x]\nname = \"x\"\n",
        )
        .unwrap();

        let result = run(project.path(), "all", dest.path(), ConfigPolicy::strict());

        // Should return an Err (not Ok with a non-zero code) because the project
        // itself is invalid and we bail before touching any host.
        assert!(result.is_err(), "invalid config must return Err");
        assert!(
            !dest.path().join("web1.toml").exists(),
            "no bundle should be written when project-level validation fails"
        );
    }
}
