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
    pub age_identity: Option<std::path::PathBuf>,
}

/// Build a `TargetNotFound` error that names the requested selector and lists
/// the hosts and groups the project actually defines.
fn unknown_target_error(selector: &str, inventory: &Inventory) -> Error {
    let (host_names, group_names) = inventory.target_names();
    let fmt = |v: Vec<String>| {
        if v.is_empty() {
            "(none)".to_string()
        } else {
            v.join(", ")
        }
    };
    Error::TargetNotFound(format!(
        "{selector}; available hosts: {} | groups: {}",
        fmt(host_names),
        fmt(group_names),
    ))
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
    /// and every failure is a `connection` failure). The actionable signal is
    /// "could not reach the targets", which maps to exit 4.
    pub fn is_connection_only_failure(&self) -> bool {
        self.every_failure_is_kind("connection")
    }

    /// True when every host failed purely on config/parse (no host did real
    /// work and every failure is a `config` failure). The actionable signal is
    /// "the configuration is broken", which maps to exit 5 - not a connectivity
    /// or resource-execution failure.
    pub fn is_config_only_failure(&self) -> bool {
        self.every_failure_is_kind("config")
    }

    /// Shared shape for the two classifiers: no host did real work and every
    /// failed resource is of the given failure kind. A build/connection failure
    /// is matched by its `failure_kind` (the resource keeps its real type for
    /// display); the whole-host abort path, which encodes the kind directly in
    /// `resource_type`, is matched by that.
    fn every_failure_is_kind(&self, kind: &str) -> bool {
        !self.summaries.is_empty()
            && self.summaries.iter().all(|s| {
                s.summary.failed > 0
                    && s.summary.ok == 0
                    && s.summary.changed == 0
                    && s.resources
                        .iter()
                        .filter(|r| r.status == ResourceStatus::Failed)
                        .all(|r| r.failure_kind.as_deref() == Some(kind) || r.resource_type == kind)
            })
    }

    /// Compute the process exit code based on the run outcome.
    /// Failures take priority over changes.
    pub fn exit_code(&self) -> i32 {
        use crate::error::exit_codes;
        if self.is_config_only_failure() {
            return exit_codes::INVALID_CONFIG;
        }
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
        let resource_defs = crate::resource_def::load_resource_defs(
            &base_dir.join("resources"),
            crate::config::known_resource_types(),
        )?;
        let provider_defs = crate::provider_def::load_provider_defs(
            &base_dir.join("providers"),
            base_dir,
            crate::config::known_resource_types(),
            &resource_defs,
        )?;
        crate::config::validate_state_files(
            &state_files,
            self.policy,
            &resource_defs,
            &provider_defs,
        )?;
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
        let is_all = matches!(selector, crate::inventory::selector::Selector::All);

        // An empty inventory is not a bad target: the project or its inventory is
        // the problem. Say which, rather than blaming the target name. ("all" on
        // an empty inventory is a valid no-op and falls through below.)
        if inventory.hosts.is_empty() && !is_all {
            let hosts_toml = base_dir.join("hosts.toml");
            return Err(Error::Config(if hosts_toml.is_file() {
                format!("no hosts defined in {}", hosts_toml.display())
            } else {
                format!(
                    "no hosts.toml in {}; this is not a verg project directory \
                     (run verg from your project, or pass --path <dir>)",
                    base_dir.display()
                )
            }));
        }

        let hosts = match inventory.filter(&selector) {
            Ok(hosts) => hosts,
            // A selector naming an unknown host/group: re-raise with the set of
            // valid targets so the typo is immediately actionable.
            Err(Error::TargetNotFound(_)) => {
                return Err(unknown_target_error(target_selector, &inventory));
            }
            Err(e) => return Err(e),
        };

        // A non-"all" selector that matched nothing (e.g. an exclude that removed
        // everything) is still an error, with the same actionable target list.
        if hosts.is_empty() && !is_all {
            return Err(unknown_target_error(target_selector, &inventory));
        }

        if hosts.is_empty() {
            return Ok(EngineResult { summaries: vec![] });
        }

        let secrets = crate::secrets::load_secrets(base_dir, self.age_identity.as_deref())?;
        let mut ctx = inventory.to_template_context();
        crate::secrets::inject_secret_namespace(&mut ctx, secrets);
        let inventory_ctx = Arc::new(ctx);
        let state_files = Arc::new(state_files);
        let resource_defs = Arc::new(resource_defs);
        let provider_defs = Arc::new(provider_defs);

        let semaphore = Arc::new(tokio::sync::Semaphore::new(self.parallel));
        let mut join_set = JoinSet::new();

        for host in hosts {
            let host = host.clone();
            let state_files = Arc::clone(&state_files);
            let inventory_ctx = Arc::clone(&inventory_ctx);
            let resource_defs = Arc::clone(&resource_defs);
            let provider_defs = Arc::clone(&provider_defs);
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
                            changes: Vec::new(),
                            failure_kind: None,
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
                    // Build runs after preflight on purpose: it consumes the facts
                    // preflight gathered (templates and `when:` reference `fact.*`),
                    // so an unreachable host reports a connection error here rather
                    // than local build errors - the same as before partial builds.
                    let outcome = Bundle::build(&host, &state_files, &base_dir, &inventory_ctx)?;
                    let mut bundle = outcome.bundle;
                    bundle.resource_defs =
                        crate::bundle::referenced_defs(&bundle.resources, &resource_defs);
                    bundle.provider_defs =
                        crate::bundle::referenced_provider_defs(&bundle.resources, &provider_defs);
                    // Resources that failed to build (Failed) and their dependents
                    // (Skipped) are merged in below, so one unbuildable resource
                    // never hides the diff of the rest of the host.
                    let mut synthetic: Vec<ResourceResult> = outcome
                        .failures
                        .into_iter()
                        .map(|f| ResourceResult {
                            resource_type: f.resource_type,
                            name: f.name,
                            status: ResourceStatus::Failed,
                            diff: None,
                            from: None,
                            to: None,
                            output: None,
                            error: Some(f.error),
                            changes: Vec::new(),
                            // Classify by the build error's kind so exit codes match
                            // the pre-partial-build behavior, while the resource
                            // keeps its real type/name for the report.
                            failure_kind: Some(f.kind.to_string()),
                        })
                        .collect();
                    synthetic.extend(outcome.skipped.into_iter().map(|s| ResourceResult {
                        resource_type: s.resource_type,
                        name: s.name,
                        status: ResourceStatus::Skipped,
                        diff: None,
                        from: None,
                        to: None,
                        output: None,
                        error: Some(s.reason),
                        changes: Vec::new(),
                        failure_kind: None,
                    }));
                    let has_build_problems = !synthetic.is_empty();
                    // Partial execution is for read-only runs only. `apply`
                    // (dry_run=false) is strict: if the desired state could not be
                    // fully computed, the host is NOT mutated - it fails wholesale,
                    // exactly as it did before partial builds existed. A read-only
                    // diff/check/plan still executes the buildable resources for a
                    // partial diff; when nothing is buildable, skip the agent
                    // round-trip (preflight already ran to gather facts).
                    let skip_execute =
                        has_build_problems && (!dry_run || bundle.resources.is_empty());
                    let mut resources = if skip_execute {
                        Vec::new()
                    } else {
                        transport
                            .execute(&conn, &bundle, dry_run, &arch, has_version)
                            .await?
                            .summary
                            .resources
                    };
                    resources.extend(synthetic);
                    Ok::<RunSummary, Error>(RunSummary::from_results(&host_name, resources))
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
                            // Classify by error kind: a bundle/parse failure is a
                            // config error, not a connection failure.
                            resource_type: e.failure_kind().into(),
                            name: host_name.clone(),
                            status: ResourceStatus::Failed,
                            diff: None,
                            from: None,
                            to: None,
                            output: None,
                            error: Some(e.to_string()),
                            changes: Vec::new(),
                            failure_kind: None,
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
                            resource_type: "internal".into(),
                            name: "task".into(),
                            status: ResourceStatus::Failed,
                            diff: None,
                            from: None,
                            to: None,
                            output: None,
                            error: Some(format!("task join error: {e}")),
                            changes: Vec::new(),
                            failure_kind: None,
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
                changes: Vec::new(),
                failure_kind: None,
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
    fn config_only_failure_exits_invalid_config() {
        // A config/parse failure (e.g. a missing $env var during bundle build)
        // exits as INVALID_CONFIG, not as a connection error.
        let r = EngineResult {
            summaries: vec![failed_summary("a", "config")],
        };
        assert!(r.is_config_only_failure());
        assert!(!r.is_connection_only_failure());
        assert_eq!(r.exit_code(), crate::error::exit_codes::INVALID_CONFIG);
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
                changes: Vec::new(),
                failure_kind: None,
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
            age_identity: None,
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
            age_identity: None,
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
            age_identity: None,
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
                            changes: Vec::new(),
                            failure_kind: None,
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
                            changes: Vec::new(),
                            failure_kind: None,
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
                            changes: Vec::new(),
                            failure_kind: None,
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
            age_identity: None,
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

    /// Like `mock_engine` but with a caller-supplied state file, so a test can
    /// mix resources that build with ones that fail to build.
    fn mock_engine_state(
        dir: &std::path::Path,
        addresses: &[&str],
        state_toml: &str,
        transport: MockTransport,
    ) -> Engine<MockTransport> {
        let mut hosts_toml = String::new();
        for addr in addresses {
            let name = addr.replace(['.', ':'], "_");
            hosts_toml.push_str(&format!("[hosts.{name}]\naddress = \"{addr}\"\n"));
        }
        std::fs::write(dir.join("hosts.toml"), &hosts_toml).unwrap();
        std::fs::create_dir_all(dir.join("state")).unwrap();
        std::fs::write(dir.join("state").join("base.toml"), state_toml).unwrap();
        Engine {
            transport,
            parallel: 8,
            policy: crate::config::ConfigPolicy::strict(),
            timeout_secs: 30,
            age_identity: None,
        }
    }

    #[tokio::test]
    async fn engine_merges_build_failures_with_executed_results() {
        // A host with a buildable resource AND one that fails to build (undefined
        // var) must report BOTH: the executed result and a per-resource build
        // failure - the build failure never hides the rest of the diff.
        let dir = tempfile::TempDir::new().unwrap();
        let transport = MockTransport::new("0.0.0");
        transport.set_outcome(
            "192.0.2.1",
            MockOutcome::Succeed {
                changed: 0,
                ok: 1,
                failed: 0,
            },
        );
        let state = "[resource.pkg.curl]\nname = \"curl\"\n\n[resource.file.bad]\ncontent = \"{{ undefined_var }}\"\n";
        let engine = mock_engine_state(dir.path(), &["192.0.2.1"], state, transport);
        let result = engine.run(dir.path(), "all", true).await.unwrap();

        let host = &result.summaries[0];
        assert_eq!(
            host.summary.ok, 1,
            "the executed resource is still reported"
        );
        assert_eq!(host.summary.failed, 1, "the unbuildable resource failed");
        let failed = host
            .resources
            .iter()
            .find(|r| r.status == ResourceStatus::Failed)
            .unwrap();
        // The build failure keeps the configured resource's real identity for
        // display, but classifies as "config" for exit-code purposes.
        assert_eq!(failed.resource_type, "file");
        assert_eq!(failed.name, "bad");
        assert_eq!(failed.failure_kind.as_deref(), Some("config"));
    }

    #[tokio::test]
    async fn engine_all_unbuildable_exits_invalid_config() {
        // When every selected resource fails to build, the run still classifies as
        // a config-only failure (exit INVALID_CONFIG), even though each failed
        // resource keeps its real type - the failure_kind carries the config
        // classification. Guards the pre-partial-build exit-code contract.
        let dir = tempfile::TempDir::new().unwrap();
        let transport = MockTransport::new("0.0.0");
        let state = "[resource.file.a]\ncontent = \"{{ nope_a }}\"\n\n[resource.file.b]\ncontent = \"{{ nope_b }}\"\n";
        let engine = mock_engine_state(dir.path(), &["192.0.2.1"], state, transport);
        let result = engine.run(dir.path(), "all", true).await.unwrap();

        assert!(result.is_config_only_failure());
        assert_eq!(
            result.exit_code(),
            crate::error::exit_codes::INVALID_CONFIG,
            "all-unbuildable must exit INVALID_CONFIG, not total/partial failure"
        );
    }

    #[tokio::test]
    async fn engine_apply_is_strict_on_build_failures() {
        // apply (dry_run=false) must NOT execute a partial bundle: if any resource
        // fails to build, the host is not mutated at all. The mock would report a
        // change if execute ran, so a change appearing would be the regression.
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
        let state = "[resource.pkg.curl]\nname = \"curl\"\n\n[resource.file.bad]\ncontent = \"{{ undefined_var }}\"\n";
        let engine = mock_engine_state(dir.path(), &["192.0.2.1"], state, transport);
        // dry_run = false => apply.
        let result = engine.run(dir.path(), "all", false).await.unwrap();

        let host = &result.summaries[0];
        assert_eq!(
            host.summary.changed, 0,
            "apply must not execute the partial bundle after a build failure"
        );
        assert_eq!(
            host.summary.failed, 1,
            "the build failure is still reported"
        );
        assert!(
            host.resources
                .iter()
                .all(|r| r.status != ResourceStatus::Changed),
            "nothing may be applied when the desired state is incomplete"
        );
    }

    #[tokio::test]
    async fn engine_skips_ssh_when_all_resources_fail_to_build() {
        // If every in-scope resource fails to build, the engine must not connect.
        // The mock is set to FAIL on execute; a connection error would appear iff
        // execute ran. The build failure (real type, not "connection") plus the
        // absence of the connection error prove SSH was skipped.
        let dir = tempfile::TempDir::new().unwrap();
        let transport = MockTransport::new("0.0.0");
        transport.set_outcome("192.0.2.1", MockOutcome::Fail("connection refused".into()));
        let state = "[resource.file.bad]\ncontent = \"{{ undefined_var }}\"\n";
        let engine = mock_engine_state(dir.path(), &["192.0.2.1"], state, transport);
        let result = engine.run(dir.path(), "all", true).await.unwrap();

        let host = &result.summaries[0];
        assert_eq!(host.summary.failed, 1);
        let failed = host
            .resources
            .iter()
            .find(|r| r.status == ResourceStatus::Failed)
            .unwrap();
        assert_eq!(
            failed.resource_type, "file",
            "the build failure, not a probe"
        );
        assert_ne!(failed.resource_type, "connection");
        assert!(
            !failed
                .error
                .as_deref()
                .unwrap_or_default()
                .contains("refused"),
            "execute must not have run: {:?}",
            failed.error
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
