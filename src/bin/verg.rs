use std::path::PathBuf;
use std::process;

use clap::Parser;

use verg::commands;
use verg::engine::Engine;
use verg::error::Error;
use verg::output::{OutputConfig, OutputFormat};
use verg::transport::HostKeyChecking;
use verg::transport::ssh::SshTransport;

#[derive(Parser)]
#[command(
    name = "verg",
    version,
    about = "Desired-state infrastructure convergence engine"
)]
struct Cli {
    #[arg(long, short = 'o', global = true, default_value = "auto", value_enum)]
    output: OutputFormat,

    /// Emit JSON output (alias for --output=json)
    #[arg(long, global = true, hide = true)]
    json: bool,

    /// Suppress per-resource lines; print only the final summary
    #[arg(long, short = 'q', global = true)]
    quiet: bool,

    /// Proceed when stdin is not a TTY (required for CI/pipelines)
    #[arg(long, short = 'y', global = true)]
    yes: bool,

    /// Project directory. Defaults to the nearest ancestor containing hosts.toml
    /// (or a verg/ subdirectory), discovered by walking up from the current dir.
    #[arg(long, env = "VERG_PATH", global = true)]
    path: Option<PathBuf>,

    #[arg(long, default_value = "10", global = true, value_parser = clap::value_parser!(u16).range(1..))]
    parallel: u16,

    /// Path to SSH config file
    #[arg(long, env = "VERG_SSH_CONFIG", global = true)]
    ssh_config: Option<PathBuf>,

    /// Directory containing verg-agent binaries per architecture
    #[arg(long, env = "VERG_AGENT_DIR", global = true)]
    agent_dir: Option<PathBuf>,

    /// Downgrade unknown-key, unknown-type, and wrong-type config errors to warnings
    #[arg(long, global = true)]
    lax_config: bool,

    /// SSH host key checking policy
    #[arg(long, global = true, default_value = "yes", value_enum)]
    host_key_checking: HostKeyChecking,

    /// Path to a known_hosts file for host key verification
    #[arg(long, global = true)]
    ssh_known_hosts: Option<PathBuf>,

    /// Skip agent binary checksum verification (for air-gapped or local builds)
    #[arg(long, global = true)]
    skip_agent_checksum: bool,

    /// Path to an age identity file for decrypting verg/secrets.age
    #[arg(long, env = "VERG_AGE_IDENTITY", global = true)]
    age_identity: Option<PathBuf>,

    /// Per-host timeout in seconds (a hung host fails instead of blocking the run)
    #[arg(long, default_value = "600", global = true)]
    timeout: u64,

    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Converge targets to desired state
    Apply {
        /// Hosts/groups to converge (e.g. all, web, prod:!db). Required - no default, to prevent accidental mass applies.
        #[arg(long, short)]
        targets: String,
    },
    /// Show what would change without applying
    Diff {
        /// Target pattern to match hosts (default: all)
        #[arg(long, short, default_value = "all")]
        targets: String,

        #[arg(long, default_value = "100")]
        limit: usize,

        #[arg(long, default_value = "0")]
        offset: usize,

        #[arg(long)]
        fields: Option<String>,
    },
    /// Verify targets match desired state
    Check {
        /// Target pattern to match hosts (default: all)
        #[arg(long, short, default_value = "all")]
        targets: String,
    },
    /// Audit committed config for $env secret references (read-only)
    Lint,
    /// Print resource type schemas as JSON
    Schema,
    /// Scaffold a new verg project directory
    Init {
        /// Overwrite existing scaffold files
        #[arg(long)]
        force: bool,
    },
    /// Generate shell completions
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Build per-host bundles offline and write them to a directory (for pull-mode agents)
    Publish {
        /// Hosts/groups to publish bundles for (e.g. all, web, prod:!db)
        #[arg(long, short)]
        targets: String,
        /// Directory to write the per-host bundle files into
        #[arg(long)]
        dest: PathBuf,
    },
}

#[tokio::main]
async fn main() {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            // Help and version requests are not errors; let clap handle them normally.
            if e.kind() == clap::error::ErrorKind::DisplayHelp
                || e.kind() == clap::error::ErrorKind::DisplayVersion
            {
                e.exit();
            }
            // Clap parse errors (unknown subcommand, missing required arg, etc.)
            // emit the structured error envelope as the last line of stderr so
            // consumers can branch on `kind` without parsing prose.
            let envelope = serde_json::json!({
                "error": {
                    "kind": "invalid_config",
                    "message": e.to_string().trim().to_string()
                }
            });
            // Print clap's human-friendly message first, then the envelope last.
            eprint!("{e}");
            eprintln!(
                "{}",
                serde_json::to_string(&envelope).unwrap_or_else(|_| {
                    r#"{"error":{"kind":"internal_error","message":"serialization failed"}}"#
                        .to_string()
                })
            );
            process::exit(verg::error::exit_codes::INVALID_CONFIG);
        }
    };
    let output = OutputConfig::new(cli.output.clone(), cli.json, cli.quiet);

    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel_watch = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            cancel_watch.store(true, std::sync::atomic::Ordering::SeqCst);
            eprintln!("interrupt received: finishing in-flight hosts, skipping the rest");
        }
    });

    let code = match run(cli, &output, cancel.clone()).await {
        Ok(code) => code,
        Err(e) => {
            let envelope = serde_json::json!({
                "error": {
                    "kind": e.kind_str(),
                    "message": e.to_string()
                }
            });
            eprintln!(
                "{}",
                serde_json::to_string(&envelope).unwrap_or_else(|_| {
                    r#"{"error":{"kind":"internal_error","message":"serialization failed"}}"#
                        .to_string()
                })
            );
            e.exit_code()
        }
    };
    let code = if cancel.load(std::sync::atomic::Ordering::SeqCst) {
        130
    } else {
        code
    };
    process::exit(code);
}

struct EngineConfig {
    parallel: usize,
    ssh_config: Option<PathBuf>,
    agent_dir: Option<PathBuf>,
    policy: verg::config::ConfigPolicy,
    host_key_checking: HostKeyChecking,
    known_hosts: Option<PathBuf>,
    skip_agent_checksum: bool,
    timeout_secs: u64,
    age_identity: Option<PathBuf>,
}

async fn run(
    cli: Cli,
    output: &OutputConfig,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<i32, Error> {
    let policy = if cli.lax_config {
        verg::config::ConfigPolicy::lax()
    } else {
        verg::config::ConfigPolicy::strict()
    };

    let engine_config = EngineConfig {
        parallel: cli.parallel.into(),
        ssh_config: cli.ssh_config.clone(),
        agent_dir: cli.agent_dir.clone(),
        policy,
        host_key_checking: cli.host_key_checking,
        known_hosts: cli.ssh_known_hosts.clone(),
        skip_agent_checksum: cli.skip_agent_checksum,
        timeout_secs: cli.timeout,
        age_identity: cli.age_identity.clone(),
    };

    match cli.command {
        Command::Apply { targets } => {
            let base_dir = resolve_project_dir(cli.path.clone())?;
            let engine = build_engine(engine_config)?;
            commands::apply::run(&engine, &base_dir, &targets, cli.yes, output, cancel).await
        }
        Command::Diff {
            targets,
            limit,
            offset,
            fields,
        } => {
            let base_dir = resolve_project_dir(cli.path.clone())?;
            let engine = build_engine(engine_config)?;
            commands::diff::run(
                &engine,
                &base_dir,
                &targets,
                commands::diff::DiffOptions {
                    limit,
                    offset,
                    fields,
                },
                output,
                cancel,
            )
            .await
        }
        Command::Check { targets } => {
            let base_dir = resolve_project_dir(cli.path.clone())?;
            let engine = build_engine(engine_config)?;
            commands::check::run(&engine, &base_dir, &targets, output, cancel).await
        }
        Command::Lint => {
            let base_dir = resolve_project_dir(cli.path.clone())?;
            commands::lint::run(&base_dir, output)
        }
        Command::Schema => {
            // Schema works with built-in types outside a project; discover one
            // if present so custom resource/provider defs are included.
            let base_dir = discover_project_dir(cli.path.clone());
            let custom_defs = verg::resource_def::load_resource_defs(
                &base_dir.join("resources"),
                verg::config::known_resource_types(),
            )?;
            let provider_defs = verg::provider_def::load_provider_defs(
                &base_dir.join("providers"),
                &base_dir,
                verg::config::known_resource_types(),
                &custom_defs,
            )?;
            verg::schema::run(&custom_defs, &provider_defs);
            Ok(0)
        }
        Command::Init { force } => {
            // Init creates a project; it must not discover an existing one.
            let base_dir = cli.path.clone().unwrap_or_else(|| PathBuf::from("verg"));
            commands::init::run(&base_dir, force)?;
            Ok(0)
        }
        Command::Completions { shell } => {
            use clap::CommandFactory;
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "verg", &mut std::io::stdout());
            Ok(0)
        }
        Command::Publish { targets, dest } => {
            let base_dir = resolve_project_dir(cli.path.clone())?;
            commands::publish::run(
                &base_dir,
                &targets,
                &dest,
                policy,
                cli.age_identity.as_deref(),
            )
        }
    }
}

/// Resolve the project directory for a command that requires one. An explicit
/// `--path`/`VERG_PATH` wins; otherwise the project root is discovered by
/// walking up from the current directory. Fails with a clear message when no
/// project is found, instead of silently loading an empty inventory.
fn resolve_project_dir(explicit: Option<PathBuf>) -> Result<PathBuf, Error> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    let cwd = std::env::current_dir()
        .map_err(|e| Error::Other(format!("failed to read current directory: {e}")))?;
    verg::config::discover_project_root(&cwd).ok_or_else(|| {
        Error::Config(format!(
            "no verg project found: no hosts.toml in {} or any parent directory \
             (nor in a verg/ subdirectory). Run verg from your project directory, \
             or pass --path <dir>.",
            cwd.display()
        ))
    })
}

/// Like `resolve_project_dir`, but never fails: used by `schema`, which is
/// useful even outside a project (built-in types only). Falls back to `verg`.
fn discover_project_dir(explicit: Option<PathBuf>) -> PathBuf {
    explicit
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .and_then(|cwd| verg::config::discover_project_root(&cwd))
        })
        .unwrap_or_else(|| PathBuf::from("verg"))
}

fn build_engine(cfg: EngineConfig) -> Result<Engine, Error> {
    let agent_dir = match cfg.agent_dir {
        Some(dir) => dir,
        None => {
            // Default: look next to the verg binary, then ~/.local/share/verg/agents/
            let exe_dir = std::env::current_exe()
                .map_err(|e| Error::Other(format!("failed to get current exe: {e}")))?;
            let beside_exe = exe_dir.parent().map(|p| p.join("agents"));
            if beside_exe.as_ref().is_some_and(|p| p.is_dir()) {
                beside_exe.unwrap()
            } else {
                dirs::data_dir()
                    .unwrap_or_else(|| PathBuf::from("/usr/local/share"))
                    .join("verg")
                    .join("agents")
            }
        }
    };

    let version = env!("CARGO_PKG_VERSION").to_string();

    let mut transport = SshTransport::new(agent_dir, version);
    transport.ssh_config = cfg.ssh_config;
    transport.host_key_checking = cfg.host_key_checking;
    transport.known_hosts = cfg.known_hosts;
    transport.skip_agent_checksum = cfg.skip_agent_checksum;

    Ok(Engine {
        transport,
        parallel: cfg.parallel,
        policy: cfg.policy,
        timeout_secs: cfg.timeout_secs,
        age_identity: cfg.age_identity,
    })
}
