use clap::Parser;
use std::process;
use verg::error::Error;
use verg::output::OutputConfig;

#[derive(Parser)]
#[command(name = "verg", version, about = "Desired-state infrastructure convergence engine")]
struct Cli {
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    /// Converge targets to desired state
    Apply,
    /// Show what would change without applying
    Diff,
    /// Verify targets match desired state
    Check,
    /// Print resource type schemas as JSON
    Schema,
    /// Scaffold a new verg project directory
    Init,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let _output = OutputConfig::new(cli.json);

    if let Err(e) = run(cli).await {
        eprintln!("Error: {e}");
        process::exit(e.exit_code());
    }
}

async fn run(cli: Cli) -> Result<(), Error> {
    match cli.command {
        Command::Apply => todo!(),
        Command::Diff => todo!(),
        Command::Check => todo!(),
        Command::Schema => todo!(),
        Command::Init => todo!(),
    }
}
