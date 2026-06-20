use std::path::Path;

fn main() {
    // Embed the agent checksum manifest if the release build provides one.
    // Local cargo builds leave it empty, which disables checksum verification.
    // Written to OUT_DIR (not cargo:rustc-env) because the manifest is multi-line
    // and cargo parses build-script output line-by-line.
    println!("cargo:rerun-if-env-changed=VERG_AGENT_CHECKSUMS_FILE");
    let manifest = std::env::var("VERG_AGENT_CHECKSUMS_FILE")
        .ok()
        .filter(|p| !p.is_empty())
        .and_then(|p| {
            println!("cargo:rerun-if-changed={p}");
            std::fs::read_to_string(Path::new(&p)).ok()
        })
        .unwrap_or_default();
    let out_dir = std::env::var("OUT_DIR").expect("OUT_DIR set by cargo");
    std::fs::write(Path::new(&out_dir).join("agent_checksums.txt"), manifest)
        .expect("write agent_checksums.txt");
}
