use crate::error::Error;

use super::{ResolvedResource, ResourceResult, run_checked, run_cmd};

fn user_exists(name: &str) -> Result<bool, Error> {
    let output = run_cmd("id", &[name])?;
    Ok(output.status.success())
}

/// Parse a `getent passwd` line into `(home, shell)`.
///
/// The format is `name:password:uid:gid:gecos:home:shell` (7 colon-separated
/// fields). Returns `None` when the line has fewer than 7 fields.
fn parse_passwd_line(line: &str) -> Option<(String, String)> {
    let mut fields = line.splitn(7, ':');
    // Skip: name, password, uid, gid, gecos
    fields.next()?; // name
    fields.next()?; // password
    fields.next()?; // uid
    fields.next()?; // gid
    fields.next()?; // gecos
    let home = fields.next()?.to_string();
    let shell = fields.next()?.trim_end().to_string();
    Some((home, shell))
}

/// Returns `true` when the desired supplementary group set differs from the
/// current supplementary group set.
///
/// `id_ng`  - output of `id -nG <user>` (all groups, space-separated, incl. primary)
/// `id_gn`  - output of `id -gn <user>` (primary group only)
/// `desired_csv` - comma-separated desired SUPPLEMENTARY groups (e.g. "alice,dev")
///
/// The primary group is excluded from the comparison because `usermod -G` only
/// sets the supplementary set; the primary group is not in scope.
fn supplementary_groups_differ(id_ng: &str, id_gn: &str, desired_csv: &str) -> bool {
    let primary = id_gn.trim();
    let all: std::collections::HashSet<&str> = id_ng.split_whitespace().collect();
    let current_supplementary: std::collections::HashSet<&str> =
        all.into_iter().filter(|g| *g != primary).collect();

    let desired: std::collections::HashSet<&str> = if desired_csv.trim().is_empty() {
        std::collections::HashSet::new()
    } else {
        desired_csv.split(',').map(str::trim).collect()
    };

    current_supplementary != desired
}

/// Build the `usermod` flag list for the attributes that differ.
///
/// Only attributes that the resource explicitly sets (non-`None`) are compared.
/// Returns the flags (without the trailing `name` argument) as a `Vec<String>`.
fn usermod_flags(
    current_home: &str,
    current_shell: &str,
    desired_home: Option<&str>,
    desired_shell: Option<&str>,
    groups_differ: bool,
    desired_groups: Option<&str>,
) -> Vec<String> {
    let mut flags: Vec<String> = Vec::new();

    if let Some(shell) = desired_shell
        && shell != current_shell
    {
        flags.push("-s".into());
        flags.push(shell.into());
    }

    if let Some(home) = desired_home
        && home != current_home
    {
        flags.push("-d".into());
        flags.push(home.into());
        flags.push("-m".into());
    }

    if groups_differ && let Some(groups) = desired_groups {
        flags.push("-G".into());
        flags.push(groups.into());
    }

    flags
}

pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let name = resource.prop_str_required("name")?;

    let state = resource.prop_str_or("state", "present");

    let exists = user_exists(name)?;

    match (state, exists) {
        ("present", false) => {
            // User does not exist: create it.
            if dry_run {
                return Ok(ResourceResult::changed(
                    "user",
                    resource.name.clone(),
                    format!("would create user {name}"),
                ));
            }
            let mut args = vec!["--system"];
            if let Some(home) = resource.prop_str("home") {
                args.extend(["-d", home, "-m"]);
            }
            if let Some(shell) = resource.prop_str("shell") {
                args.extend(["-s", shell]);
            }
            if let Some(groups) = resource.prop_str("groups") {
                args.extend(["-G", groups]);
            }
            args.push(name);
            let output = run_cmd("useradd", &args)?;
            if output.status.success() {
                Ok(ResourceResult::changed(
                    "user",
                    resource.name.clone(),
                    format!("created user {name}"),
                ))
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(Error::Resource(format!("useradd failed: {stderr}")))
            }
        }

        ("present", true) => {
            // User exists: compare current attributes to desired and update if needed.
            let desired_shell = resource.prop_str("shell");
            let desired_home = resource.prop_str("home");
            let desired_groups = resource.prop_str("groups");

            // Read current home and shell from getent passwd.
            let passwd_out = run_cmd("getent", &["passwd", name])?;
            let passwd_line = String::from_utf8_lossy(&passwd_out.stdout);
            let (current_home, current_shell) =
                parse_passwd_line(passwd_line.trim()).ok_or_else(|| {
                    Error::Resource(format!(
                        "could not parse getent passwd output for user {name}"
                    ))
                })?;

            // Determine if groups differ, only when the resource sets `groups`.
            let groups_differ = if desired_groups.is_some() {
                let all_out = run_cmd("id", &["-nG", name])?;
                let all_groups = String::from_utf8_lossy(&all_out.stdout);
                let primary_out = run_cmd("id", &["-gn", name])?;
                let primary_group = String::from_utf8_lossy(&primary_out.stdout);
                supplementary_groups_differ(
                    all_groups.trim(),
                    primary_group.trim(),
                    desired_groups.unwrap_or(""),
                )
            } else {
                false
            };

            let flags = usermod_flags(
                &current_home,
                &current_shell,
                desired_home,
                desired_shell,
                groups_differ,
                desired_groups,
            );

            if flags.is_empty() {
                return Ok(ResourceResult::ok("user", resource.name.clone()));
            }

            // Describe what will change.
            let mut changes: Vec<String> = Vec::new();
            if desired_shell
                .map(|s| s != current_shell.as_str())
                .unwrap_or(false)
            {
                changes.push(format!(
                    "shell {} -> {}",
                    current_shell,
                    desired_shell.unwrap_or("")
                ));
            }
            if desired_home
                .map(|h| h != current_home.as_str())
                .unwrap_or(false)
            {
                changes.push(format!(
                    "home {} -> {}",
                    current_home,
                    desired_home.unwrap_or("")
                ));
            }
            if groups_differ {
                changes.push(format!("groups -> {}", desired_groups.unwrap_or("")));
            }

            if dry_run {
                return Ok(ResourceResult::from_changes(
                    "user",
                    resource.name.clone(),
                    &changes,
                ));
            }

            // Build the argv: flags + name.
            let mut args: Vec<&str> = flags.iter().map(String::as_str).collect();
            args.push(name);
            run_checked("usermod", &args, "usermod")?;

            Ok(ResourceResult::from_changes(
                "user",
                resource.name.clone(),
                &changes,
            ))
        }

        ("absent", true) => {
            if dry_run {
                return Ok(ResourceResult::changed(
                    "user",
                    resource.name.clone(),
                    format!("would remove user {name}"),
                ));
            }
            let output = run_cmd("userdel", &["-r", name])?;
            if output.status.success() {
                Ok(ResourceResult::changed(
                    "user",
                    resource.name.clone(),
                    format!("removed user {name}"),
                ))
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(Error::Resource(format!("userdel failed: {stderr}")))
            }
        }

        _ => Ok(ResourceResult::ok("user", resource.name.clone())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn resource(props: HashMap<String, toml::Value>) -> ResolvedResource {
        crate::resources::test_resource("user", "t", props)
    }

    #[test]
    fn missing_name_is_an_error() {
        let err = execute(&resource(HashMap::new()), true).unwrap_err();
        assert!(err.to_string().contains("requires 'name'"), "got: {err}");
    }

    // --- parse_passwd_line ---

    #[test]
    fn parse_passwd_line_extracts_home_and_shell() {
        let line = "alice:x:1001:1001:Alice:/home/alice:/bin/bash";
        let (home, shell) = parse_passwd_line(line).unwrap();
        assert_eq!(home, "/home/alice");
        assert_eq!(shell, "/bin/bash");
    }

    #[test]
    fn parse_passwd_line_trims_trailing_newline() {
        let line = "alice:x:1001:1001::/home/alice:/bin/sh\n";
        let (home, shell) = parse_passwd_line(line).unwrap();
        assert_eq!(home, "/home/alice");
        assert_eq!(shell, "/bin/sh");
    }

    #[test]
    fn parse_passwd_line_empty_gecos() {
        let line = "daemon:x:1:1::/var/spool/daemon:/bin/false";
        let (home, shell) = parse_passwd_line(line).unwrap();
        assert_eq!(home, "/var/spool/daemon");
        assert_eq!(shell, "/bin/false");
    }

    #[test]
    fn parse_passwd_line_returns_none_for_too_few_fields() {
        assert!(parse_passwd_line("alice:x:1001").is_none());
    }

    // --- supplementary_groups_differ ---

    #[test]
    fn supplementary_groups_differ_same_set_is_idempotent() {
        // Primary is "primary"; supplementary are alice and dev.
        // The desired set matches exactly -> no diff.
        assert!(!supplementary_groups_differ(
            "primary alice dev",
            "primary",
            "alice,dev"
        ));
    }

    #[test]
    fn supplementary_groups_differ_order_does_not_matter() {
        // Reversed order in desired CSV -> still idempotent.
        assert!(!supplementary_groups_differ(
            "primary alice dev",
            "primary",
            "dev,alice"
        ));
    }

    #[test]
    fn supplementary_groups_differ_missing_desired_group() {
        // Current supplementary: {alice, dev}; desired: {alice} -> differs.
        assert!(supplementary_groups_differ(
            "primary alice dev",
            "primary",
            "alice"
        ));
    }

    #[test]
    fn supplementary_groups_differ_extra_desired_group() {
        // Current supplementary: {alice, dev}; desired: {alice, dev, extra} -> differs.
        assert!(supplementary_groups_differ(
            "primary alice dev",
            "primary",
            "alice,dev,extra"
        ));
    }

    #[test]
    fn supplementary_groups_differ_primary_only_empty_desired_is_idempotent() {
        // User belongs only to its primary group; desired supplementary is empty -> no diff.
        assert!(!supplementary_groups_differ("primary", "primary", ""));
    }

    #[test]
    fn supplementary_groups_differ_primary_excluded_from_comparison() {
        // "primary" is in id_ng but NOT in the desired set; must not be treated as drift.
        assert!(!supplementary_groups_differ(
            "primary dev",
            "primary",
            "dev"
        ));
    }

    // --- usermod_flags ---

    #[test]
    fn usermod_flags_empty_when_nothing_differs() {
        let flags = usermod_flags("/home/alice", "/bin/bash", None, None, false, None);
        assert!(flags.is_empty());
    }

    #[test]
    fn usermod_flags_shell_change() {
        let flags = usermod_flags(
            "/home/alice",
            "/bin/sh",
            None,
            Some("/bin/bash"),
            false,
            None,
        );
        assert_eq!(flags, vec!["-s", "/bin/bash"]);
    }

    #[test]
    fn usermod_flags_home_change() {
        let flags = usermod_flags(
            "/home/old",
            "/bin/bash",
            Some("/home/new"),
            None,
            false,
            None,
        );
        assert_eq!(flags, vec!["-d", "/home/new", "-m"]);
    }

    #[test]
    fn usermod_flags_groups_change() {
        let flags = usermod_flags(
            "/home/alice",
            "/bin/bash",
            None,
            None,
            true,
            Some("docker,sudo"),
        );
        assert_eq!(flags, vec!["-G", "docker,sudo"]);
    }

    #[test]
    fn usermod_flags_all_changes() {
        let flags = usermod_flags(
            "/home/old",
            "/bin/sh",
            Some("/home/new"),
            Some("/bin/bash"),
            true,
            Some("docker"),
        );
        assert_eq!(
            flags,
            vec!["-s", "/bin/bash", "-d", "/home/new", "-m", "-G", "docker"]
        );
    }

    #[test]
    fn usermod_flags_no_change_when_values_match() {
        // Shell matches: no flag emitted.
        let flags = usermod_flags(
            "/home/alice",
            "/bin/bash",
            Some("/home/alice"),
            Some("/bin/bash"),
            false,
            None,
        );
        assert!(flags.is_empty());
    }
}
