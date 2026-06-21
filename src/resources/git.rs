use std::path::Path;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, run_checked, run_cmd};

/// Returns true when `r` looks like a git object SHA: all hexadecimal digits,
/// length 7 to 40 inclusive. Git accepts abbreviated SHAs (7+ chars) for
/// checkout; a full SHA is 40 chars.
///
/// Known limitation: a branch or tag whose name is itself all-hex and 7-40
/// chars (e.g. "deadbeef") is classified as a SHA, so `clone` omits `--branch`
/// for it. Convergence is unaffected (the post-clone `checkout <ref>` resolves
/// the tag/branch); only the clone is a full default-branch clone rather than a
/// branch-scoped one. There is no way to disambiguate without querying the repo.
pub fn is_sha(r: &str) -> bool {
    let len = r.len();
    (7..=40).contains(&len) && r.chars().all(|c| c.is_ascii_hexdigit())
}

/// Build the argv for `git clone` with all options before the url and path
/// arguments, as git requires.
///
/// - `--branch <ref>` is included only when `gitref` is Some AND is not a SHA
///   (git clone --branch accepts branch names and tags, not raw SHAs).
/// - `--depth <n>` is included when `depth` is Some.
pub fn clone_args(url: &str, path: &str, gitref: Option<&str>, depth: Option<&str>) -> Vec<String> {
    let mut args = vec!["clone".to_string()];

    if let Some(r) = gitref
        && !is_sha(r)
    {
        args.push("--branch".to_string());
        args.push(r.to_string());
    }

    if let Some(d) = depth {
        args.push("--depth".to_string());
        args.push(d.to_string());
    }

    args.push(url.to_string());
    args.push(path.to_string());

    args
}

pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    let url = resource.prop_str_required("url")?;
    let path = resource.prop_str_required("path")?;
    let gitref = resource.prop_str("ref");
    let depth = resource.prop_str("depth");
    let state = resource.prop_str_or("state", "present");
    let name = resource.name.clone();

    let mut changes: Vec<String> = Vec::new();

    match state {
        "absent" => {
            if Path::new(path).exists() {
                changes.push(format!("remove {path}"));
                if !dry_run {
                    std::fs::remove_dir_all(path)
                        .map_err(|e| Error::Resource(format!("failed to remove {path}: {e}")))?;
                }
            }
        }
        "present" => {
            let is_repo = run_cmd("git", &["-C", path, "rev-parse", "--verify", "HEAD"])
                .map(|o| o.status.success())
                .unwrap_or(false);

            if !is_repo {
                // Detect the case where path exists but is not a git repo: a
                // regular file, or a non-empty directory. git clone would fail
                // cryptically in both cases, so fail cleanly instead.
                let p = Path::new(path);
                let is_file = p.is_file();
                let nonempty_dir = std::fs::read_dir(path)
                    .map(|mut d| d.next().is_some())
                    .unwrap_or(false);
                if is_file || nonempty_dir {
                    return Ok(ResourceResult::failed(
                        "git",
                        name,
                        "path exists and is not a git repository",
                    ));
                }

                if dry_run {
                    changes.push(format!("would clone {url} -> {path}"));
                } else {
                    let args_owned = clone_args(url, path, gitref, depth);
                    let args_refs: Vec<&str> = args_owned.iter().map(String::as_str).collect();
                    run_checked("git", &args_refs, "git clone")?;

                    // When a SHA ref was given, --branch was skipped; checkout the SHA now.
                    if let Some(r) = gitref
                        && is_sha(r)
                    {
                        run_checked("git", &["-C", path, "checkout", r], "git checkout")?;
                    }

                    changes.push(format!("cloned {url} -> {path}"));
                }
            } else {
                // The path is already a git repo.
                if dry_run {
                    // Compare LOCALLY (no fetch, so dry-run stays side-effect-free
                    // and network-free): if the checkout is already at the desired
                    // ref as known locally (origin/<ref> from the last clone/fetch),
                    // report no change. This avoids flagging a converged checkout as
                    // drift on every diff/check. Remote drift since the last fetch is
                    // not detected here (apply fetches and reconciles).
                    let ref_display = gitref.unwrap_or("HEAD");
                    let desired = resolve_desired_sha(path, gitref).ok();
                    let current = resolve_sha(path, "HEAD").ok();
                    let converged = matches!((&desired, &current), (Some(d), Some(c)) if d == c);
                    if !converged {
                        changes.push(format!("would update {path} to {ref_display}"));
                    }
                } else {
                    // Fetch the latest from origin (including tags).
                    run_checked(
                        "git",
                        &["-C", path, "fetch", "--tags", "origin"],
                        "git fetch",
                    )?;

                    // Determine the desired SHA. For branch names the freshly
                    // fetched remote-tracking ref is more accurate than the
                    // (possibly stale) local branch.
                    let desired_sha = resolve_desired_sha(path, gitref)?;
                    let current_sha = resolve_sha(path, "HEAD")?;

                    if desired_sha != current_sha {
                        let ref_display = gitref.unwrap_or("HEAD");
                        changes.push(format!("checkout {ref_display}"));

                        let checkout_target = gitref.unwrap_or("HEAD");
                        run_checked(
                            "git",
                            &["-C", path, "checkout", checkout_target],
                            "git checkout",
                        )?;
                        // Hard-reset to the resolved SHA so we land at exactly
                        // the desired commit regardless of local state.
                        run_checked(
                            "git",
                            &["-C", path, "reset", "--hard", &desired_sha],
                            "git reset",
                        )?;
                    }
                }
            }
        }
        other => {
            return Err(Error::Resource(format!(
                "git resource: unknown state '{other}'; expected 'present' or 'absent'"
            )));
        }
    }

    Ok(ResourceResult::from_changes("git", name, &changes))
}

/// Resolve `git rev-parse <spec>` to a full SHA string.
fn resolve_sha(path: &str, spec: &str) -> Result<String, Error> {
    let output = run_cmd("git", &["-C", path, "rev-parse", "--verify", spec])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Resource(format!(
            "failed to resolve git ref '{spec}': {stderr}"
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Determine the desired commit SHA after a fetch.
///
/// For branch-like refs we prefer `origin/<ref>` (the freshly fetched remote
/// tracking ref) over the local `<ref>` (which may lag behind). If
/// `origin/<ref>` does not resolve (e.g. the ref is a tag or SHA), we fall
/// back to `<ref>` directly.
fn resolve_desired_sha(path: &str, gitref: Option<&str>) -> Result<String, Error> {
    match gitref {
        None => resolve_sha(path, "FETCH_HEAD"),
        Some(r) => {
            let remote_ref = format!("origin/{r}");
            let output = run_cmd("git", &["-C", path, "rev-parse", "--verify", &remote_ref])?;
            if output.status.success() {
                Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
            } else {
                // Tag or SHA: resolve directly.
                resolve_sha(path, r)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_resource(props: HashMap<String, toml::Value>) -> ResolvedResource {
        crate::resources::test_resource("git", "test-repo", props)
    }

    // --- is_sha ---

    #[test]
    fn is_sha_accepts_40_hex() {
        assert!(is_sha("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2"));
    }

    #[test]
    fn is_sha_accepts_7_hex() {
        assert!(is_sha("abc1234"));
    }

    #[test]
    fn is_sha_rejects_branch_name() {
        assert!(!is_sha("main"));
    }

    #[test]
    fn is_sha_rejects_non_hex_chars() {
        // 'z' is not a hex digit, length is 7.
        assert!(!is_sha("abcz123"));
    }

    #[test]
    fn is_sha_rejects_too_short() {
        // 6 hex chars is below the 7-char minimum.
        assert!(!is_sha("abc123"));
    }

    #[test]
    fn is_sha_rejects_too_long() {
        // 41 hex chars exceeds the 40-char maximum.
        assert!(!is_sha("a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c"));
    }

    #[test]
    fn is_sha_rejects_tag_like_name() {
        assert!(!is_sha("v1.2.3"));
    }

    #[test]
    fn is_sha_treats_all_hex_name_as_sha_known_limitation() {
        // A tag/branch literally named in all-hex (7-40 chars) is classified as
        // a SHA. This is the documented heuristic limit; convergence still works
        // because the post-clone checkout resolves the ref. Pin the behavior so
        // a future change to the heuristic is a conscious decision.
        assert!(is_sha("deadbeef"));
    }

    // --- clone_args ---

    #[test]
    fn clone_args_no_ref_no_depth() {
        let args = clone_args("https://example.com/repo.git", "/tmp/repo", None, None);
        assert_eq!(
            args,
            vec!["clone", "https://example.com/repo.git", "/tmp/repo"]
        );
    }

    #[test]
    fn clone_args_branch_ref_included() {
        let args = clone_args(
            "https://example.com/repo.git",
            "/tmp/repo",
            Some("main"),
            None,
        );
        assert_eq!(
            args,
            vec![
                "clone",
                "--branch",
                "main",
                "https://example.com/repo.git",
                "/tmp/repo"
            ]
        );
    }

    #[test]
    fn clone_args_sha_ref_no_branch_flag() {
        // SHA refs must NOT get --branch; the flag only accepts branch/tag names.
        let args = clone_args(
            "https://example.com/repo.git",
            "/tmp/repo",
            Some("abc1234"),
            None,
        );
        assert_eq!(
            args,
            vec!["clone", "https://example.com/repo.git", "/tmp/repo"]
        );
    }

    #[test]
    fn clone_args_depth_included() {
        let args = clone_args("https://example.com/repo.git", "/tmp/repo", None, Some("1"));
        assert_eq!(
            args,
            vec![
                "clone",
                "--depth",
                "1",
                "https://example.com/repo.git",
                "/tmp/repo"
            ]
        );
    }

    #[test]
    fn clone_args_branch_and_depth_options_before_url() {
        let args = clone_args(
            "https://example.com/repo.git",
            "/tmp/repo",
            Some("main"),
            Some("1"),
        );
        // Options must precede url and path.
        let url_pos = args
            .iter()
            .position(|a| a == "https://example.com/repo.git")
            .unwrap();
        let depth_pos = args.iter().position(|a| a == "--depth").unwrap();
        let branch_pos = args.iter().position(|a| a == "--branch").unwrap();
        assert!(branch_pos < url_pos, "--branch must appear before url");
        assert!(depth_pos < url_pos, "--depth must appear before url");
        assert_eq!(
            args,
            vec![
                "clone",
                "--branch",
                "main",
                "--depth",
                "1",
                "https://example.com/repo.git",
                "/tmp/repo"
            ]
        );
    }

    // --- missing required props ---

    #[test]
    fn missing_url_returns_error() {
        let mut props = HashMap::new();
        props.insert("path".into(), toml::Value::String("/tmp/repo".into()));
        let resource = make_resource(props);
        let result = execute(&resource, true);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("requires 'url'"));
    }

    #[test]
    fn missing_path_returns_error() {
        let mut props = HashMap::new();
        props.insert(
            "url".into(),
            toml::Value::String("https://example.com/repo.git".into()),
        );
        let resource = make_resource(props);
        let result = execute(&resource, true);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("requires 'path'"));
    }

    // NOTE: Tests that require a live git binary (clone/fetch/checkout) are
    // covered by the e2e test suite (make e2e / Task 5). The pure-logic helpers
    // above are sufficient for unit coverage.
}
