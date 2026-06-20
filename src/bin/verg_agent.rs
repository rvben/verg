fn main() {
    let dry_run = std::env::args().any(|a| a == "--dry-run");

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
