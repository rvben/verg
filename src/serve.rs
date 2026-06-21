use std::path::Path;
use std::time::Duration;

use chrono::Utc;

use crate::bundle::Bundle;
use crate::changelog::write_serve_report;
use crate::error::Error;
use crate::resources::RunSummary;

/// Resolve the polling interval for serve mode.
///
/// When `once` is true, the interval is ignored and `Ok(None)` is returned.
/// When `once` is false, `interval` is required; a missing value or a zero
/// duration are both rejected with a config error.
pub fn resolve_interval(interval: Option<&str>, once: bool) -> Result<Option<Duration>, Error> {
    if once {
        return Ok(None);
    }
    let s = interval.ok_or_else(|| {
        Error::Config(
            "--interval is required when --once is not set; pass e.g. --interval 5m".to_string(),
        )
    })?;
    let d = crate::duration::parse_duration(s)?;
    if d.is_zero() {
        return Err(Error::Config(
            "--interval must be greater than zero; a zero duration would busy-loop".to_string(),
        ));
    }
    Ok(Some(d))
}

/// Fetch the bundle text from `source`.
///
/// If `source` starts with `http://` or `https://`, curl is used to fetch it.
/// curl's exit code is checked explicitly because `run_cmd` does not error on
/// a non-zero exit; a 404 or connect failure would otherwise be silently
/// parsed as a bundle. On a non-zero curl exit, an error is returned that
/// includes curl's stderr. On success, stdout is decoded as UTF-8.
///
/// For any other `source`, the value is treated as a local filesystem path and
/// read directly.
/// Maximum bundle size accepted in pull mode. Matches the agent's stdin cap so
/// a runaway or hostile bundle source cannot OOM the agent.
const MAX_BUNDLE_BYTES: usize = 64 * 1024 * 1024;

fn fetch_bundle_text(source: &str) -> Result<String, Error> {
    if source.starts_with("http://") {
        // A published bundle carries decrypted secrets in plaintext; over plain
        // http they can be read or replaced (MITM) in transit.
        eprintln!(
            "warning: fetching the bundle over plain http is insecure - the bundle \
             contains decrypted secrets and can be read or replaced in transit; \
             prefer https or a local path"
        );
        fetch_http(source)
    } else if source.starts_with("https://") {
        fetch_http(source)
    } else {
        let file = std::fs::File::open(source)
            .map_err(|e| Error::Config(format!("failed to read bundle from {source}: {e}")))?;
        crate::resources::read_bounded(file, MAX_BUNDLE_BYTES)
    }
}

fn fetch_http(source: &str) -> Result<String, Error> {
    let max = MAX_BUNDLE_BYTES.to_string();
    let output = crate::resources::run_cmd("curl", &["-fsSL", "--max-filesize", &max, source])?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Config(format!(
            "curl failed to fetch {source}: {stderr}"
        )));
    }
    if output.stdout.len() > MAX_BUNDLE_BYTES {
        return Err(Error::Config(format!(
            "bundle from {source} exceeds {MAX_BUNDLE_BYTES} bytes"
        )));
    }
    String::from_utf8(output.stdout)
        .map_err(|e| Error::Parse(format!("bundle from {source} is not valid UTF-8: {e}")))
}

/// Fetch a bundle, converge once, write a redacted report, and return the summary.
///
/// The report filename is `<report_dir>/<timestamp>-serve.json`. The timestamp
/// uses `%Y-%m-%dT%H-%M-%S` (colons replaced with hyphens) so the filename is
/// valid on all platforms.
pub fn serve_once(source: &str, report_dir: &Path) -> Result<RunSummary, Error> {
    let text = fetch_bundle_text(source)?;
    let bundle = Bundle::from_toml(&text)?;
    let summary = crate::agent::execute_bundle(bundle, false)?;
    let now = Utc::now();
    let nanos = now.timestamp_subsec_nanos();
    let timestamp = format!("{}-{nanos:09}", now.format("%Y-%m-%dT%H-%M-%S"));
    write_serve_report(report_dir, &summary, source, &timestamp)?;
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ---------------------------------------------------------------------------
    // resolve_interval
    // ---------------------------------------------------------------------------

    #[test]
    fn resolve_interval_once_true_returns_none() {
        assert_eq!(resolve_interval(None, true).unwrap(), None);
        assert_eq!(resolve_interval(Some("5m"), true).unwrap(), None);
        assert_eq!(resolve_interval(Some("0s"), true).unwrap(), None);
    }

    #[test]
    fn resolve_interval_once_false_with_valid_interval() {
        let result = resolve_interval(Some("5m"), false).unwrap();
        assert_eq!(result, Some(Duration::from_secs(300)));
    }

    #[test]
    fn resolve_interval_once_false_with_seconds() {
        let result = resolve_interval(Some("30s"), false).unwrap();
        assert_eq!(result, Some(Duration::from_secs(30)));
    }

    #[test]
    fn resolve_interval_once_false_missing_interval_errors() {
        let err = resolve_interval(None, false).unwrap_err();
        assert!(
            matches!(err, Error::Config(_)),
            "expected Config error, got: {err}"
        );
    }

    #[test]
    fn resolve_interval_once_false_zero_duration_errors() {
        let err = resolve_interval(Some("0s"), false).unwrap_err();
        assert!(
            matches!(err, Error::Config(_)),
            "expected Config error for zero duration, got: {err}"
        );
    }

    // ---------------------------------------------------------------------------
    // serve_once - local file source
    // ---------------------------------------------------------------------------

    /// Build a minimal bundle TOML that writes a file to `target_path`.
    fn minimal_file_bundle_toml(host: &str, target_path: &str, content: &str) -> String {
        // Escape the content and path for TOML inline strings.
        // We use a simple format that avoids characters needing TOML escaping
        // in the test strings we pass.
        format!(
            r#"host = "{host}"

[[resources]]
resource_type = "file"
name = "test-file"
after = []
notify = []
handler = false
sensitive = false

[resources.props]
path = "{target_path}"
content = "{content}"
"#
        )
    }

    #[test]
    fn serve_once_local_file_creates_report_and_returns_summary() {
        let tmp = TempDir::new().unwrap();
        let target_file = tmp.path().join("output.txt");
        let report_dir = tmp.path().join("reports");

        // Write bundle TOML to a temp file.
        let bundle_toml =
            minimal_file_bundle_toml("test-host", target_file.to_str().unwrap(), "hello serve");
        let bundle_path = tmp.path().join("bundle.toml");
        std::fs::write(&bundle_path, &bundle_toml).unwrap();

        let source = bundle_path.to_str().unwrap();
        let summary = serve_once(source, &report_dir).unwrap();

        // The file resource should have converged (Changed on first run).
        assert_eq!(summary.host, "test-host");
        assert_eq!(summary.resources.len(), 1);
        assert!(
            summary.summary.changed > 0 || summary.summary.ok > 0,
            "expected at least one changed or ok resource, got: {:?}",
            summary.summary
        );

        // A report file must exist in report_dir.
        let report_files: Vec<_> = std::fs::read_dir(&report_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with("-serve.json"))
            .collect();
        assert_eq!(report_files.len(), 1, "expected exactly one serve report");

        // The report must be valid JSON.
        let report_content = std::fs::read_to_string(report_files[0].path()).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&report_content).unwrap();
        assert_eq!(parsed["source"], source);
    }

    #[test]
    fn serve_once_idempotent_second_call_returns_ok() {
        let tmp = TempDir::new().unwrap();
        let target_file = tmp.path().join("output2.txt");
        let report_dir = tmp.path().join("reports2");

        let bundle_toml = minimal_file_bundle_toml(
            "test-host",
            target_file.to_str().unwrap(),
            "idempotent content",
        );
        let bundle_path = tmp.path().join("bundle2.toml");
        std::fs::write(&bundle_path, &bundle_toml).unwrap();

        let source = bundle_path.to_str().unwrap();

        // First call: file does not exist -> Changed.
        let s1 = serve_once(source, &report_dir).unwrap();
        assert!(
            s1.summary.changed > 0 || s1.summary.ok > 0,
            "first call should change or ok"
        );

        // Second call: file already has the right content -> Ok (no change).
        let s2 = serve_once(source, &report_dir).unwrap();
        assert_eq!(
            s2.summary.changed, 0,
            "second call must not change an already-converged file"
        );
        assert_eq!(s2.summary.failed, 0);

        // Both reports must be present.
        let report_files: Vec<_> = std::fs::read_dir(&report_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with("-serve.json"))
            .collect();
        assert_eq!(report_files.len(), 2, "expected two serve reports");
    }

    #[test]
    fn serve_once_nonexistent_source_returns_err() {
        let tmp = TempDir::new().unwrap();
        let report_dir = tmp.path().join("reports");
        let result = serve_once("/nonexistent/path/bundle.toml", &report_dir);
        assert!(
            result.is_err(),
            "expected Err for non-existent source, got Ok"
        );
    }

    #[test]
    fn serve_once_invalid_toml_returns_err() {
        let tmp = TempDir::new().unwrap();
        let report_dir = tmp.path().join("reports");
        let bundle_path = tmp.path().join("bad.toml");
        std::fs::write(&bundle_path, "this is not valid bundle toml!!!").unwrap();
        let result = serve_once(bundle_path.to_str().unwrap(), &report_dir);
        assert!(result.is_err(), "expected Err for invalid TOML bundle");
    }

    #[test]
    fn serve_once_runs_native_provider_from_bundle() {
        use crate::bundle::Bundle;
        use crate::provider_def::ProviderDef;
        use crate::resources::{ResolvedResource, ResourceStatus};
        use std::collections::HashMap;

        let dir = tempfile::tempdir().unwrap();
        let mut provider_defs = HashMap::new();
        provider_defs.insert(
            "myprov".to_string(),
            ProviderDef {
                description: String::new(),
                interpreter: vec!["/bin/sh".into()],
                source: r#"printf '%s' '{"status":"changed","diff":"pulled"}'"#.into(),
                params: HashMap::new(),
            },
        );
        let bundle = Bundle {
            host: "h".into(),
            resources: vec![ResolvedResource {
                resource_type: "myprov".into(),
                name: "t".into(),
                props: HashMap::new(),
                after: vec![],
                notify: vec![],
                when: None,
                handler: false,
                register: None,
                sensitive: false,
            }],
            facts: HashMap::new(),
            resource_defs: HashMap::new(),
            provider_defs,
        };
        let bundle_path = dir.path().join("bundle.toml");
        std::fs::write(&bundle_path, bundle.to_toml().unwrap()).unwrap();

        let summary = serve_once(bundle_path.to_str().unwrap(), dir.path()).unwrap();
        assert_eq!(summary.resources.len(), 1);
        assert_eq!(summary.resources[0].status, ResourceStatus::Changed);
        assert_eq!(summary.resources[0].diff.as_deref(), Some("pulled"));
    }

    // Note: the https fetch path is NOT unit-tested here because it requires
    // an actual network call (or a mock HTTP server). Code structure ensures
    // that the curl exit-code check is always applied. Coverage of the https
    // path is provided by the e2e test suite.
}
