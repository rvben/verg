use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::task::JoinSet;

use crate::bundle::Bundle;
use crate::error::Error;
use crate::inventory::{Inventory, selector};
use crate::resources::{ResourceResult, ResourceStatus, RunSummary};
use crate::state;
use crate::transport::Transport;
use crate::transport::ssh::{HostConn, SshTransport};

pub struct Engine<T: Transport = SshTransport> {
    pub transport: T,
    pub parallel: usize,
    pub policy: crate::config::ConfigPolicy,
    pub timeout_secs: u64,
}

#[derive(Debug)]
pub struct EngineResult {
    pub summaries: Vec<RunSummary>,
}

impl EngineResult {
    pub fn has_failures(&self) -> bool {
        self.summaries.iter().any(|s| s.summary.failed > 0)
    }

    pub fn has_changes(&self) -> bool {
        self.summaries.iter().any(|s| s.summary.changed > 0)
    }

    /// True when every host failed purely on connectivity (no host did real work
    /// and every failure is a `connection`-type resource). The actionable signal
    /// is "could not reach the targets", which maps to exit 4.
    pub fn is_connection_only_failure(&self) -> bool {
        !self.summaries.is_empty()
            && self.summaries.iter().all(|s| {
                s.summary.failed > 0
                    && s.summary.ok == 0
                    && s.summary.changed == 0
                    && s.resources
                        .iter()
                        .filter(|r| r.status == ResourceStatus::Failed)
                        .all(|r| r.resource_type == "connection")
            })
    }

    /// Compute the process exit code based on the run outcome.
    /// Failures take priority over changes.
    pub fn exit_code(&self) -> i32 {
        use crate::error::exit_codes;
        if self.is_connection_only_failure() {
            return crate::error::exit_codes::CONNECTION_ERROR;
        }
        if self.has_failures() {
            if self.has_changes() || self.summaries.iter().any(|s| s.summary.ok > 0) {
                exit_codes::PARTIAL_FAILURE
            } else {
                exit_codes::TOTAL_FAILURE
            }
        } else if self.has_changes() {
            exit_codes::SUCCESS
        } else {
            exit_codes::NOTHING_CHANGED
        }
    }
}

impl<T: Transport + Send + Sync + 'static> Engine<T> {
    pub async fn run(
        &self,
        base_dir: &Path,
        target_selector: &str,
        dry_run: bool,
    ) -> Result<EngineResult, Error> {
        self.run_cancellable(
            base_dir,
            target_selector,
            dry_run,
            Arc::new(AtomicBool::new(false)),
        )
        .await
    }

    pub async fn run_cancellable(
        &self,
        base_dir: &Path,
        target_selector: &str,
        dry_run: bool,
        cancel: Arc<AtomicBool>,
    ) -> Result<EngineResult, Error> {
        let inventory = Inventory::load(base_dir)?;

        // Validate config on the control host before anything host-specific, so
        // typos fail locally and loudly even if the selector matches no hosts.
        let state_dir = base_dir.join("state");
        let state_files = state::load_state_dir(&state_dir)?;
        crate::config::validate_state_files(&state_files, self.policy)?;
        if state_dir.is_dir() {
            let mut entries: Vec<_> = std::fs::read_dir(&state_dir)
                .map_err(|e| Error::Config(format!("failed to read {}: {e}", state_dir.display())))?
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
                .map(|e| e.path())
                .collect();
            entries.sort();
            for path in entries {
                let raw = std::fs::read_to_string(&path).map_err(|e| {
                    Error::Config(format!("failed to read {}: {e}", path.display()))
                })?;
                let source = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
                crate::config::validate_state_file_toml(&raw, &source, self.policy)?;
            }
        }

        let selector = selector::parse_selector(target_selector)?;
        let hosts = inventory.filter(&selector)?;

        // A non-empty selector that matches nothing is an error. The "all"
        // selector on an empty inventory is valid (nothing to do).
        if hosts.is_empty() && !matches!(selector, crate::inventory::selector::Selector::All) {
            return Err(Error::TargetNotFound(target_selector.into()));
        }

        if hosts.is_empty() {
            return Ok(EngineResult { summaries: vec![] });
        }

        let inventory_ctx = Arc::new(inventory.to_template_context());
        let state_files = Arc::new(state_files);

        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.parallel));
        let mut join_set = JoinSet::new();

        for host in hosts {
            let host = host.clone();
            let state_files = Arc::clone(&state_files);
            let inventory_ctx = Arc::clone(&inventory_ctx);
            let transport = self.transport.for_host();
            let sem = semaphore.clone();
            let cancel = cancel.clone();

            let base_dir = base_dir.to_path_buf();
            let timeout_secs = self.timeout_secs;
            join_set.spawn(async move {
                let _permit = sem.acquire().await.expect("semaphore is never closed");
                if cancel.load(Ordering::SeqCst) {
                    return RunSummary::from_results(
                        &host.name,
                        vec![ResourceResult {
                            resource_type: "connection".into(),
                            name: host.name.clone(),
                            status: ResourceStatus::Skipped,
                            diff: None,
                            from: None,
                            to: None,
                            output: None,
                            error: Some("cancelled before start".into()),
                        }],
                    );
                }
                let host_name = host.name.clone();
                let host_user = host.user.clone();
                let host_address = host.address.clone();
                let host_port = host.port;
                let work = async {
                    let conn = HostConn {
                        user: &host.user,
                        address: &host.address,
                        port: host.port,
                    };

                    // One SSH round-trip gathers both system facts and the
                    // installed agent version stamp, eliminating a second hop.
                    let (facts, remote_version) = transport.preflight(&conn).await?;

                    let arch = facts
                        .get("fact.arch")
                        .cloned()
                        .unwrap_or_else(|| "x86_64".into());

                    // Version matches when the remote stamp (trimmed) equals the
                    // running verg version. Missing or empty means push is needed.
                    let has_version = crate::transport::ssh::version_matches(
                        remote_version.as_deref().unwrap_or(""),
                        transport.current_version(),
                    );

                    let mut host = host;
                    // Inject facts as variables (fact.arch, fact.hostname, etc.)
                    for (k, v) in &facts {
                        host.vars
                            .entry(k.clone())
                            .or_insert_with(|| toml::Value::String(v.clone()));
                    }
                    // Inject group membership as variables (group.docker = "true")
                    for group in &host.groups {
                        host.vars
                            .entry(format!("group.{group}"))
                            .or_insert_with(|| toml::Value::String("true".into()));
                    }

                    let conn = HostConn {
                        user: &host.user,
                        address: &host.address,
                        port: host.port,
                    };
                    let bundle = Bundle::build(&host, &state_files, &base_dir, &inventory_ctx)?;
                    let result = transport
                        .execute(&conn, &bundle, dry_run, &arch, has_version)
                        .await?;
                    Ok::<RunSummary, Error>(result.summary)
                };
                let result =
                    match tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), work)
                        .await
                    {
                        Ok(inner) => inner,
                        Err(_elapsed) => Err(Error::Connection(format!(
                            "host timed out after {timeout_secs}s"
                        ))),
                    };

                // Best-effort teardown: close the ControlMaster socket so the
                // background master exits immediately (rather than lingering
                // for the ControlPersist duration). Done after all work for
                // this host is complete, so no in-flight session uses the socket.
                transport.teardown_control_master(&HostConn {
                    user: &host_user,
                    address: &host_address,
                    port: host_port,
                });

                match result {
                    Ok(summary) => summary,
                    Err(e) => RunSummary::from_results(
                        &host_name,
                        vec![ResourceResult {
                            resource_type: "connection".into(),
                            name: host_name.clone(),
                            status: ResourceStatus::Failed,
                            diff: None,
                            from: None,
                            to: None,
                            output: None,
                            error: Some(e.to_string()),
                        }],
                    ),
                }
            });
        }

        let mut summaries = Vec::new();
        while let Some(result) = join_set.join_next().await {
            match result {
                Ok(summary) => summaries.push(summary),
                Err(e) => {
                    summaries.push(RunSummary::from_results(
                        "unknown",
                        vec![ResourceResult {
                            resource_type: "connection".into(),
                            name: "task".into(),
                            status: ResourceStatus::Failed,
                            diff: None,
                            from: None,
                            to: None,
                            output: None,
                            error: Some(format!("task join error: {e}")),
                        }],
                    ));
                }
            }
        }

        Ok(EngineResult { summaries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    fn failed_summary(host: &str, rtype: &str) -> RunSummary {
        RunSummary::from_results(
            host,
            vec![ResourceResult {
                resource_type: rtype.into(),
                name: host.into(),
                status: ResourceStatus::Failed,
                diff: None,
                from: None,
                to: None,
                output: None,
                error: Some("boom".into()),
            }],
        )
    }

    #[test]
    fn connection_only_failure_exits_connection_error() {
        let r = EngineResult {
            summaries: vec![
                failed_summary("a", "connection"),
                failed_summary("b", "connection"),
            ],
        };
        assert!(r.is_connection_only_failure());
        assert_eq!(r.exit_code(), crate::error::exit_codes::CONNECTION_ERROR);
    }

    #[test]
    fn resource_failure_is_not_connection_error() {
        let r = EngineResult {
            summaries: vec![failed_summary("a", "pkg")],
        };
        assert!(!r.is_connection_only_failure());
        assert_ne!(r.exit_code(), crate::error::exit_codes::CONNECTION_ERROR);
    }

    #[test]
    fn one_good_host_plus_one_unreachable_is_not_connection_only() {
        // A host that succeeded (or did nothing) alongside an unreachable host is
        // a PARTIAL situation, not a pure connection failure.
        let ok = RunSummary::from_results(
            "a",
            vec![ResourceResult {
                resource_type: "pkg".into(),
                name: "x".into(),
                status: ResourceStatus::Ok,
                diff: None,
                from: None,
                to: None,
                output: None,
                error: None,
            }],
        );
        let r = EngineResult {
            summaries: vec![ok, failed_summary("b", "connection")],
        };
        assert!(!r.is_connection_only_failure());
    }

    #[tokio::test]
    async fn precancelled_run_skips_all_hosts() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("hosts.toml"),
            "[hosts.web1]\naddress = \"192.0.2.10\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("state")).unwrap();
        std::fs::write(
            dir.path().join("state").join("base.toml"),
            "[resource.pkg.curl]\nname = \"curl\"\n",
        )
        .unwrap();

        let engine = Engine {
            transport: SshTransport::new(std::path::PathBuf::from("/tmp"), "0.0.0".into()),
            parallel: 1,
            policy: crate::config::ConfigPolicy::strict(),
            timeout_secs: 600,
        };
        let cancel = Arc::new(AtomicBool::new(true)); // already cancelled
        let result = engine
            .run_cancellable(dir.path(), "all", true, cancel)
            .await
            .unwrap();
        // The single host was skipped (no SSH attempted), so no failures.
        assert!(!result.has_failures(), "should skip, not fail");
        assert_eq!(
            result.summaries[0].resources[0].status,
            ResourceStatus::Skipped
        );
    }

    #[tokio::test]
    async fn run_rejects_typoed_state_key_before_ssh() {
        let dir = tempfile::TempDir::new().unwrap();
        // RFC 5737 TEST-NET-1 address; never actually contacted because
        // validation fails first.
        std::fs::write(
            dir.path().join("hosts.toml"),
            "[hosts.web1]\naddress = \"192.0.2.10\"\n",
        )
        .unwrap();
        let state = dir.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        std::fs::write(
            state.join("web.toml"),
            "targetss = [\"web\"]\n[resource.pkg.nginx]\nname = \"nginx\"\n",
        )
        .unwrap();

        let engine = Engine {
            transport: SshTransport::new(std::path::PathBuf::from("/tmp"), "0.0.0".into()),
            parallel: 1,
            policy: crate::config::ConfigPolicy::strict(),
            timeout_secs: 600,
        };
        let err = engine.run(dir.path(), "all", true).await.unwrap_err();
        assert_eq!(
            err.exit_code(),
            crate::error::exit_codes::INVALID_CONFIG,
            "typoed top-level key must fail as invalid_config, got: {err}"
        );
    }

    #[tokio::test]
    async fn host_timeout_produces_failed_summary() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join("hosts.toml"),
            "[hosts.web1]\naddress = \"192.0.2.10\"\n",
        )
        .unwrap();
        std::fs::create_dir_all(dir.path().join("state")).unwrap();
        std::fs::write(
            dir.path().join("state").join("base.toml"),
            "[resource.pkg.curl]\nname = \"curl\"\n",
        )
        .unwrap();

        let engine = Engine {
            transport: SshTransport::new(std::path::PathBuf::from("/tmp"), "0.0.0".into()),
            parallel: 1,
            policy: crate::config::ConfigPolicy::strict(),
            timeout_secs: 1,
        };
        let start = std::time::Instant::now();
        let result = engine.run(dir.path(), "all", true).await.unwrap();
        // The 1s tokio timeout fires before ssh's ConnectTimeout=10 to the
        // non-routable TEST-NET address, proving the per-host timeout works.
        assert_eq!(result.summaries.len(), 1);
        let err = result.summaries[0].resources[0]
            .error
            .as_deref()
            .unwrap_or("");
        assert!(
            err.contains("timed out"),
            "expected a timeout error, got: {err}"
        );
        assert!(
            start.elapsed().as_secs() < 8,
            "timeout should fire well before ssh ConnectTimeout"
        );
    }

    // --- MockTransport for engine orchestration tests ---

    /// Outcome injected per host into MockTransport.
    #[derive(Clone)]
    enum MockOutcome {
        /// The execute call returns a summary with the given changed/ok/failed counts.
        Succeed { changed: u32, ok: u32, failed: u32 },
        /// The execute call returns an error (maps to a connection failure summary).
        Fail(String),
    }

    /// In-test transport that returns canned results without touching SSH.
    #[derive(Clone)]
    struct MockTransport {
        /// Maps host address to the canned outcome for that host.
        outcomes: Arc<Mutex<HashMap<String, MockOutcome>>>,
        version: String,
    }

    impl MockTransport {
        fn new(version: impl Into<String>) -> Self {
            Self {
                outcomes: Arc::new(Mutex::new(HashMap::new())),
                version: version.into(),
            }
        }

        fn set_outcome(&self, address: impl Into<String>, outcome: MockOutcome) {
            self.outcomes
                .lock()
                .unwrap()
                .insert(address.into(), outcome);
        }
    }

    impl Transport for MockTransport {
        fn for_host(&self) -> Self {
            self.clone()
        }

        fn current_version(&self) -> &str {
            &self.version
        }

        async fn preflight(
            &self,
            conn: &HostConn<'_>,
        ) -> Result<(HashMap<String, String>, Option<String>), Error> {
            let mut facts = HashMap::new();
            facts.insert("fact.arch".into(), "x86_64".into());
            facts.insert("fact.hostname".into(), conn.address.to_string());
            Ok((facts, Some(self.version.clone())))
        }

        async fn execute(
            &self,
            conn: &HostConn<'_>,
            _bundle: &Bundle,
            _dry_run: bool,
            _arch: &str,
            _has_version: bool,
        ) -> Result<crate::transport::ExecResult, Error> {
            let outcome = self
                .outcomes
                .lock()
                .unwrap()
                .get(conn.address)
                .cloned()
                .unwrap_or(MockOutcome::Succeed {
                    changed: 0,
                    ok: 1,
                    failed: 0,
                });

            match outcome {
                MockOutcome::Succeed {
                    changed,
                    ok,
                    failed,
                } => {
                    let mut results = Vec::new();
                    for i in 0..changed {
                        results.push(ResourceResult {
                            resource_type: "pkg".into(),
                            name: format!("changed-{i}"),
                            status: ResourceStatus::Changed,
                            diff: None,
                            from: None,
                            to: None,
                            output: None,
                            error: None,
                        });
                    }
                    for i in 0..ok {
                        results.push(ResourceResult {
                            resource_type: "pkg".into(),
                            name: format!("ok-{i}"),
                            status: ResourceStatus::Ok,
                            diff: None,
                            from: None,
                            to: None,
                            output: None,
                            error: None,
                        });
                    }
                    for i in 0..failed {
                        results.push(ResourceResult {
                            resource_type: "pkg".into(),
                            name: format!("failed-{i}"),
                            status: ResourceStatus::Failed,
                            diff: None,
                            from: None,
                            to: None,
                            output: None,
                            error: Some("mock failure".into()),
                        });
                    }
                    let summary = RunSummary::from_results(conn.address, results);
                    Ok(crate::transport::ExecResult { summary })
                }
                MockOutcome::Fail(msg) => Err(Error::Connection(msg)),
            }
        }

        fn teardown_control_master(&self, _conn: &HostConn<'_>) {
            // No-op for mock.
        }
    }

    /// Builds a minimal verg directory with the given host addresses and a
    /// trivial state file, then returns an Engine backed by the mock transport.
    fn mock_engine(
        dir: &std::path::Path,
        addresses: &[&str],
        transport: MockTransport,
    ) -> Engine<MockTransport> {
        let mut hosts_toml = String::new();
        for addr in addresses {
            let name = addr.replace(['.', ':'], "_");
            hosts_toml.push_str(&format!("[hosts.{name}]\naddress = \"{addr}\"\n"));
        }
        std::fs::write(dir.join("hosts.toml"), &hosts_toml).unwrap();
        std::fs::create_dir_all(dir.join("state")).unwrap();
        std::fs::write(
            dir.join("state").join("base.toml"),
            "[resource.pkg.curl]\nname = \"curl\"\n",
        )
        .unwrap();

        Engine {
            transport,
            parallel: 8,
            policy: crate::config::ConfigPolicy::strict(),
            timeout_secs: 30,
        }
    }

    #[tokio::test]
    async fn engine_partial_failure_exits_partial_failure() {
        let dir = tempfile::TempDir::new().unwrap();
        let transport = MockTransport::new("0.0.0");
        // host-a succeeds with one ok result; host-b fails.
        transport.set_outcome(
            "192.0.2.1",
            MockOutcome::Succeed {
                changed: 0,
                ok: 1,
                failed: 0,
            },
        );
        transport.set_outcome("192.0.2.2", MockOutcome::Fail("connection refused".into()));

        let engine = mock_engine(dir.path(), &["192.0.2.1", "192.0.2.2"], transport);
        let result = engine.run(dir.path(), "all", true).await.unwrap();

        assert_eq!(result.summaries.len(), 2);
        assert!(result.has_failures(), "one host should have failed");
        assert_eq!(
            result.exit_code(),
            crate::error::exit_codes::PARTIAL_FAILURE,
            "one ok host + one failing host must yield PARTIAL_FAILURE(2)"
        );
    }

    #[tokio::test]
    async fn engine_all_hosts_succeed_with_changes_exits_success() {
        let dir = tempfile::TempDir::new().unwrap();
        let transport = MockTransport::new("0.0.0");
        transport.set_outcome(
            "192.0.2.1",
            MockOutcome::Succeed {
                changed: 1,
                ok: 0,
                failed: 0,
            },
        );
        transport.set_outcome(
            "192.0.2.2",
            MockOutcome::Succeed {
                changed: 2,
                ok: 0,
                failed: 0,
            },
        );

        let engine = mock_engine(dir.path(), &["192.0.2.1", "192.0.2.2"], transport);
        let result = engine.run(dir.path(), "all", true).await.unwrap();

        assert!(!result.has_failures());
        assert!(result.has_changes());
        assert_eq!(
            result.exit_code(),
            crate::error::exit_codes::SUCCESS,
            "all changed -> SUCCESS(0)"
        );
    }

    #[tokio::test]
    async fn engine_all_hosts_nothing_changed_exits_nothing_changed() {
        let dir = tempfile::TempDir::new().unwrap();
        let transport = MockTransport::new("0.0.0");
        // Default outcome (no set_outcome) returns ok=1, changed=0, failed=0.

        let engine = mock_engine(dir.path(), &["192.0.2.1", "192.0.2.2"], transport);
        let result = engine.run(dir.path(), "all", true).await.unwrap();

        assert!(!result.has_failures());
        assert!(!result.has_changes());
        assert_eq!(
            result.exit_code(),
            crate::error::exit_codes::NOTHING_CHANGED,
            "all ok, no changes -> NOTHING_CHANGED(1)"
        );
    }
}
