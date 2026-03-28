use std::path::PathBuf;
use std::process;

use clap::Parser;

use verg::commands;
use verg::engine::Engine;
use verg::error::Error;
use verg::output::OutputConfig;
use verg::transport::ssh::SshTransport;

#[derive(Parser)]
#[command(
    name = "verg",
    version,
    about = "Desired-state infrastructure convergence engine"
)]
struct Cli {
    #[arg(long, global = true)]
    json: bool,

    #[arg(long, env = "VERG_PATH", global = true)]
    path: Option<PathBuf>,

    #[arg(long, default_value = "10", global = true)]
    parallel: usize,

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
        #[arg(long, short)]
        targets: String,
    },
    /// Verify targets match desired state (exit code only)
    Check {
        #[arg(long, short)]
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
    let cli = Cli::parse();
    let output = OutputConfig::new(cli.json);

    let code = match run(cli, &output).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: {e}");
            e.exit_code()
        }
    };
    process::exit(code);
}

async fn run(cli: Cli, output: &OutputConfig) -> Result<i32, Error> {
    let base_dir = cli.path.clone().unwrap_or_else(|| PathBuf::from("verg"));

    match cli.command {
        Command::Apply { targets } => {
            let engine = build_engine(cli.parallel)?;
            commands::apply::run(&engine, &base_dir, &targets, output).await
        }
        Command::Diff { targets } => {
            let engine = build_engine(cli.parallel)?;
            commands::diff::run(&engine, &base_dir, &targets, output).await
        }
        Command::Check { targets } => {
            let engine = build_engine(cli.parallel)?;
            commands::check::run(&engine, &base_dir, &targets, output).await
        }
        Command::Schema => {
            verg::schema::run();
            Ok(0)
        }
        Command::Init => {
            let init_path = cli.path.unwrap_or_else(|| PathBuf::from("."));
            commands::init::run(&init_path)?;
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

fn build_engine(parallel: usize) -> Result<Engine, Error> {
    let current_exe = std::env::current_exe()
        .map_err(|e| Error::Other(format!("failed to get current exe: {e}")))?;
    let agent_binary = current_exe
        .parent()
        .ok_or_else(|| Error::Other("failed to get exe directory".into()))?
        .join("verg-agent");

    let version = env!("CARGO_PKG_VERSION").to_string();

    Ok(Engine {
        transport: SshTransport::new(agent_binary, version),
        parallel,
    })
}
