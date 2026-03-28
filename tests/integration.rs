use std::process::Command;
use tempfile::TempDir;

#[test]
fn init_creates_project_structure() {
    let dir = TempDir::new().unwrap();
    let verg_dir = dir.path().join("verg");
    let output = Command::new(env!("CARGO_BIN_EXE_verg"))
        .args(["init", "--path", verg_dir.to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(verg_dir.join("hosts.toml").exists());
    assert!(verg_dir.join("state/base.toml").exists());
    assert!(verg_dir.join("groups").is_dir());
}

#[test]
fn schema_outputs_valid_json() {
    let output = Command::new(env!("CARGO_BIN_EXE_verg"))
        .args(["schema"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["tool"], "verg");
    assert!(parsed["resource_types"]["pkg"].is_object());
}

#[test]
fn completions_zsh() {
    let output = Command::new(env!("CARGO_BIN_EXE_verg"))
        .args(["completions", "zsh"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("verg"));
}
