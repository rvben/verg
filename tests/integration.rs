use std::process::Command;
use tempfile::TempDir;

#[test]
fn init_creates_project_structure() {
    let dir = TempDir::new().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_verg"))
        .args(["init", "--path", dir.path().to_str().unwrap()])
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(dir.path().join("verg/hosts.toml").exists());
    assert!(dir.path().join("verg/state/base.toml").exists());
    assert!(dir.path().join("verg/groups").is_dir());
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
