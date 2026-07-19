use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use serde::{Deserialize, Serialize};

use crate::engine::Engine;
use crate::error::Error;
use crate::output::OutputConfig;
use crate::resources::{ResourceResult, ResourceStatus, RunSummary};

pub const PLAN_VERSION: u32 = 1;

/// A saved dry-run result. `apply --plan` recomputes the diff and refuses to
/// proceed unless the pending changes still match, so you apply exactly what you
/// reviewed (no TOCTOU between plan and apply).
#[derive(Debug, Serialize, Deserialize)]
pub struct Plan {
    pub version: u32,
    pub targets: String,
    pub items: Vec<RunSummary>,
}

/// Load and validate a plan file.
pub fn load(path: &Path) -> Result<Plan, Error> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| Error::Config(format!("failed to read plan {}: {e}", path.display())))?;
    let plan: Plan = serde_json::from_str(&text)
        .map_err(|e| Error::Config(format!("invalid plan {}: {e}", path.display())))?;
    if plan.version != PLAN_VERSION {
        return Err(Error::Config(format!(
            "plan {} has version {}, but this verg understands version {PLAN_VERSION}; re-run `verg plan`",
            path.display(),
            plan.version
        )));
    }
    Ok(plan)
}

/// The part of a diff a plan commits to: per host, the pending (Changed/Failed)
/// resources, sorted. Ok/Skipped resources are ignored so an incidental status
/// flip elsewhere is not treated as drift.
fn pending_signature(items: &[RunSummary]) -> Vec<(&str, Vec<ResourceResult>)> {
    let mut hosts: Vec<(&str, Vec<ResourceResult>)> = items
        .iter()
        .map(|s| {
            let mut pending: Vec<ResourceResult> = s
                .resources
                .iter()
                .filter(|r| matches!(r.status, ResourceStatus::Changed | ResourceStatus::Failed))
                .map(|r| {
                    // Compare the full reviewed result (diff, from/to bodies,
                    // error, structured changes) EXCEPT output, which can be
                    // volatile (e.g. provider register output) and is not part
                    // of the reviewed diff.
                    let mut r = r.clone();
                    r.output = None;
                    r
                })
                .collect();
            pending.sort_by(|a, b| {
                (a.resource_type.as_str(), a.name.as_str())
                    .cmp(&(b.resource_type.as_str(), b.name.as_str()))
            });
            (s.host.as_str(), pending)
        })
        .collect();
    hosts.sort_by(|a, b| a.0.cmp(b.0));
    hosts
}

/// True when the current diff no longer matches the plan's pending changes.
///
/// Scope/limitation: the check compares every reviewed field of each pending
/// resource (status, human diff string, `from`/`to` values, error, structured
/// changes) except `output`. It reliably catches a resource entering/leaving
/// the pending set, a status change, value-level changes that appear in the
/// diff (mode, owner, package, ...), and a reviewed body a resource exposes in
/// `from`/`to` (e.g. `cron`). It does NOT catch a change to a *body* a resource
/// keeps out of every reviewed field: a `file`'s `content` reports a `content`
/// FieldChange with `from`/`to` unset (the body is written, never surfaced), so
/// two different contents serialize identically; the same holds for a redacted
/// sensitive value. Detecting that needs a per-resource content hash (and, for
/// secrets, a keyed digest) in the diff - a follow-up, tracked in
/// docs/improvements.md.
pub fn is_stale(plan: &Plan, current: &[RunSummary]) -> bool {
    pending_signature(&plan.items) != pending_signature(current)
}

pub async fn run(
    engine: &Engine,
    base_dir: &Path,
    targets: &str,
    out: &Path,
    output: &OutputConfig,
    cancel: Arc<AtomicBool>,
) -> Result<i32, Error> {
    let result = engine
        .run_cancellable(base_dir, targets, true, cancel)
        .await?;
    // A plan built from a diff that had failures (unreachable host, bad config)
    // is not a clean plan; propagate a failure exit like diff/check do, so
    // scripted flows do not treat it as a successful plan.
    let exit = if result.has_failures() {
        result.exit_code()
    } else {
        0
    };
    let plan = Plan {
        version: PLAN_VERSION,
        targets: targets.to_string(),
        items: result.summaries,
    };
    let json = serde_json::to_string_pretty(&plan)
        .map_err(|e| Error::Other(format!("failed to serialize plan: {e}")))?;
    std::fs::write(out, &json)
        .map_err(|e| Error::Config(format!("failed to write plan {}: {e}", out.display())))?;

    let changed: usize = plan.items.iter().map(|s| s.summary.changed).sum();
    let failed: usize = plan.items.iter().map(|s| s.summary.failed).sum();
    if output.json {
        let envelope = serde_json::json!({
            "plan": out.display().to_string(),
            "hosts": plan.items.len(),
            "changed": changed,
            "failed": failed,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| "{}".to_string())
        );
    } else {
        println!(
            "wrote plan to {} ({changed} pending change(s), {failed} failing, across {} host(s))",
            out.display(),
            plan.items.len()
        );
    }
    Ok(exit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(host: &str, res: Vec<ResourceResult>) -> RunSummary {
        RunSummary::from_results(host, res)
    }

    fn changed_res(name: &str, diff: &str) -> ResourceResult {
        ResourceResult::changed("file", name, diff)
    }

    #[test]
    fn identical_diff_is_not_stale() {
        let items = vec![summary("h", vec![changed_res("a", "mode 0644 -> 0600")])];
        let plan = Plan {
            version: PLAN_VERSION,
            targets: "all".into(),
            items: items.clone(),
        };
        assert!(!is_stale(&plan, &items));
    }

    #[test]
    fn changed_diff_is_stale() {
        let plan = Plan {
            version: PLAN_VERSION,
            targets: "all".into(),
            items: vec![summary("h", vec![changed_res("a", "mode 0644 -> 0600")])],
        };
        // Same resource, different pending diff -> stale.
        let current = vec![summary("h", vec![changed_res("a", "mode 0644 -> 0700")])];
        assert!(is_stale(&plan, &current));
        // An extra pending resource -> stale.
        let more = vec![summary(
            "h",
            vec![
                changed_res("a", "mode 0644 -> 0600"),
                changed_res("b", "content"),
            ],
        )];
        assert!(is_stale(&plan, &more));
    }

    #[test]
    fn ok_and_skipped_resources_do_not_count_as_drift() {
        let plan = Plan {
            version: PLAN_VERSION,
            targets: "all".into(),
            items: vec![summary("h", vec![changed_res("a", "content")])],
        };
        // Current has the same pending change plus an Ok resource: not stale.
        let current = vec![summary(
            "h",
            vec![changed_res("a", "content"), ResourceResult::ok("pkg", "x")],
        )];
        assert!(!is_stale(&plan, &current));
    }

    #[test]
    fn from_to_body_change_is_stale() {
        // A resource that carries its reviewed body in from/to (e.g. cron) must
        // count as drift when that body changes, even if the diff string is the
        // same. Guards against apply --plan writing an unreviewed body.
        let mut a = changed_res("job", "would write /etc/cron.d/job");
        a.from = Some("0 3 * * * old".into());
        a.to = Some("0 3 * * * old".into());
        let mut b = changed_res("job", "would write /etc/cron.d/job");
        b.from = Some("0 3 * * * old".into());
        b.to = Some("0 4 * * * NEW".into());
        let plan = Plan {
            version: PLAN_VERSION,
            targets: "all".into(),
            items: vec![summary("h", vec![a])],
        };
        assert!(is_stale(&plan, &[summary("h", vec![b])]));
    }

    #[test]
    fn volatile_output_is_not_drift() {
        // A provider that emits different register output between plan and apply,
        // with the same pending diff, must not be treated as drift.
        let mut a = changed_res("a", "content");
        a.output = Some("register-1".into());
        let mut b = changed_res("a", "content");
        b.output = Some("register-2".into());
        let plan = Plan {
            version: PLAN_VERSION,
            targets: "all".into(),
            items: vec![summary("h", vec![a])],
        };
        assert!(!is_stale(&plan, &[summary("h", vec![b])]));
    }

    #[test]
    fn load_rejects_unknown_version() {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().join("plan.json");
        std::fs::write(&p, r#"{"version":999,"targets":"all","items":[]}"#).unwrap();
        let err = load(&p).unwrap_err();
        assert!(err.to_string().contains("version"), "got: {err}");
    }
}
