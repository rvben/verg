use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(name = "verg-agent", version)]
struct Cli {
    /// Dry-run the bundle from stdin (push mode); ignored with a subcommand.
    #[arg(long, global = true)]
    dry_run: bool,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(clap::Subcommand)]
enum Cmd {
    /// Pull a bundle from a path or URL and converge on a schedule.
    Serve {
        /// Bundle source: a local file path or an http(s) URL.
        #[arg(long)]
        source: String,
        /// How long to wait between convergence cycles (e.g. 5m, 1h). Required
        /// unless --once is set.
        #[arg(long)]
        interval: Option<String>,
        /// Run one convergence cycle and exit.
        #[arg(long)]
        once: bool,
        /// Directory where per-run reports are written.
        #[arg(long, default_value = "/var/lib/verg/runs")]
        report_dir: PathBuf,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        None => run_stdin(cli.dry_run),
        Some(Cmd::Serve {
            source,
            interval,
            once,
            report_dir,
        }) => run_serve(&source, interval.as_deref(), once, &report_dir),
    }
}

/// Execute a bundle received on stdin (the SSH push path).
///
/// This function preserves the exact behavior of the original verg-agent main:
/// read stdin up to 64 MiB, parse as a bundle, execute, print JSON to stdout,
/// and exit with the appropriate code.
fn run_stdin(dry_run: bool) {
    let input = match verg::resources::read_bounded(std::io::stdin().lock(), 64 * 1024 * 1024) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to read stdin: {e}");
            std::process::exit(5);
        }
    };

    let bundle = match verg::bundle::Bundle::from_toml(&input) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("failed to parse bundle: {e}");
            std::process::exit(5);
        }
    };

    let summary = match verg::agent::execute_bundle(bundle, dry_run) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("dependency error: {e}");
            std::process::exit(5);
        }
    };

    match serde_json::to_string(&summary) {
        Ok(json) => println!("{json}"),
        Err(e) => {
            eprintln!("failed to serialize results: {e}");
            std::process::exit(7);
        }
    }

    if summary.summary.failed > 0 && summary.summary.ok + summary.summary.changed == 0 {
        std::process::exit(3);
    } else if summary.summary.failed > 0 {
        std::process::exit(2);
    } else if summary.summary.changed == 0 {
        std::process::exit(1);
    }
}

/// Run one or more convergence cycles from a remote or local bundle source.
fn run_serve(source: &str, interval: Option<&str>, once: bool, report_dir: &std::path::Path) {
    let interval_duration = match verg::serve::resolve_interval(interval, once) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("configuration error: {e}");
            std::process::exit(5);
        }
    };

    if once {
        match verg::serve::serve_once(source, report_dir) {
            Ok(summary) => {
                eprintln!(
                    "serve: ok={} changed={} failed={} skipped={}",
                    summary.summary.ok,
                    summary.summary.changed,
                    summary.summary.failed,
                    summary.summary.skipped,
                );
                if summary.summary.failed > 0 && summary.summary.ok + summary.summary.changed == 0 {
                    std::process::exit(3);
                } else if summary.summary.failed > 0 {
                    std::process::exit(2);
                } else if summary.summary.changed == 0 {
                    std::process::exit(1);
                }
            }
            Err(e) => {
                eprintln!("serve error: {e}");
                std::process::exit(5);
            }
        }
    } else {
        let d = interval_duration.expect("resolve_interval guarantees Some for !once");
        loop {
            match verg::serve::serve_once(source, report_dir) {
                Ok(summary) => {
                    eprintln!(
                        "serve: ok={} changed={} failed={} skipped={}",
                        summary.summary.ok,
                        summary.summary.changed,
                        summary.summary.failed,
                        summary.summary.skipped,
                    );
                }
                Err(e) => {
                    eprintln!("serve error: {e}");
                }
            }
            std::thread::sleep(d);
        }
    }
}
