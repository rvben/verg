use std::path::PathBuf;
use std::process;

use clap::Parser;

use verg::commands;
use verg::engine::Engine;
use verg::error::Error;
use verg::output::{OutputConfig, OutputFormat};
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

    #[arg(long, short = 'q', global = true)]
    quiet: bool,

    #[arg(long, short = 'y', global = true)]
    yes: bool,

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

    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Converge targets to desired state
    Apply {
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
    /// Verify targets match desired state (exit code only)
    Check {
        /// Target pattern to match hosts (default: all)
        #[arg(long, short, default_value = "all")]
        targets: String,
    },
    /// Print resource type schemas as JSON
    Schema,
    /// Scaffold a new verg project directory
    Init,
    /// Generate shell completions
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
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
            eprintln!("{}", serde_json::to_string(&envelope).unwrap());
            process::exit(e.exit_code());
        }
    };
    let output = OutputConfig::new(cli.output.clone(), cli.json);

    let code = match run(cli, &output).await {
        Ok(code) => code,
        Err(e) => {
            let envelope = serde_json::json!({
                "error": {
                    "kind": e.kind_str(),
                    "message": e.to_string()
                }
            });
            eprintln!("{}", serde_json::to_string(&envelope).unwrap());
            e.exit_code()
        }
    };
    process::exit(code);
}

async fn run(cli: Cli, output: &OutputConfig) -> Result<i32, Error> {
    let base_dir = cli.path.clone().unwrap_or_else(|| PathBuf::from("verg"));
    let policy = if cli.lax_config {
        verg::config::ConfigPolicy::lax()
    } else {
        verg::config::ConfigPolicy::strict()
    };

    match cli.command {
        Command::Apply { targets } => {
            let engine = build_engine(
                cli.parallel.into(),
                cli.ssh_config.clone(),
                cli.agent_dir.clone(),
                policy,
            )?;
            commands::apply::run(&engine, &base_dir, &targets, cli.yes, output).await
        }
        Command::Diff {
            targets,
            limit,
            offset,
            fields,
        } => {
            let engine = build_engine(
                cli.parallel.into(),
                cli.ssh_config.clone(),
                cli.agent_dir.clone(),
                policy,
            )?;
            commands::diff::run(&engine, &base_dir, &targets, limit, offset, fields, output).await
        }
        Command::Check { targets } => {
            let engine = build_engine(
                cli.parallel.into(),
                cli.ssh_config.clone(),
                cli.agent_dir.clone(),
                policy,
            )?;
            commands::check::run(&engine, &base_dir, &targets, output).await
        }
        Command::Schema => {
            verg::schema::run();
            Ok(0)
        }
        Command::Init => {
            commands::init::run(&base_dir)?;
            Ok(0)
        }
        Command::Completions { shell } => {
            use clap::CommandFactory;
            let mut cmd = Cli::command();
            clap_complete::generate(shell, &mut cmd, "verg", &mut std::io::stdout());
            Ok(0)
        }
    }
}

fn build_engine(
    parallel: usize,
    ssh_config: Option<PathBuf>,
    agent_dir: Option<PathBuf>,
    policy: verg::config::ConfigPolicy,
) -> Result<Engine, Error> {
    let agent_dir = match agent_dir {
        Some(dir) => dir,
        None => {
            // Default: look next to the verg binary, then ~/.local/share/verg/agents/
            let exe_dir = std::env::current_exe()
                .map_err(|e| Error::Other(format!("failed to get current exe: {e}")))?;
            let beside_exe = exe_dir.parent().unwrap().join("agents");
            if beside_exe.is_dir() {
                beside_exe
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
    transport.ssh_config = ssh_config;

    Ok(Engine {
        transport,
        parallel,
        policy,
    })
}
