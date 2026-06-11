use std::process::Command;
use tempfile::TempDir;

// Validates the `schema` command output against the vendored CLI Spec v0.2 JSON Schema.
#[test]
fn schema_validates_against_clispec_v0_2() {
    let output = Command::new(env!("CARGO_BIN_EXE_verg"))
        .args(["schema"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let schema_output: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("schema command must output valid JSON");

    let fixture = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/clispec-v0.2.json"
    ))
    .expect("clispec v0.2 JSON Schema fixture must be present");
    let meta_schema: serde_json::Value =
        serde_json::from_str(&fixture).expect("fixture must be valid JSON");

    let validator = jsonschema::validator_for(&meta_schema)
        .expect("clispec v0.2 JSON Schema must be a valid JSON Schema");

    let errors: Vec<String> = validator
        .iter_errors(&schema_output)
        .map(|e| e.to_string())
        .collect();

    assert!(
        errors.is_empty(),
        "schema output failed clispec v0.2 validation: {errors:?}"
    );
}

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
    assert_eq!(parsed["name"], "verg");
    assert!(parsed["resource_types"]["pkg"].is_object());
}

#[test]
fn schema_has_clispec_v0_2_fields() {
    let output = Command::new(env!("CARGO_BIN_EXE_verg"))
        .args(["schema"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();

    assert_eq!(parsed["clispec"], "0.2", "clispec version must be 0.2");
    assert_eq!(parsed["name"], "verg", "name must be verg");
    assert!(
        parsed["version"].is_string(),
        "version field must be present"
    );
    assert!(
        parsed["commands"].is_array(),
        "commands field must be an array"
    );
    assert!(
        parsed["global_args"].is_array(),
        "global_args field must be an array"
    );
    assert!(parsed["errors"].is_array(), "errors field must be an array");

    let commands = parsed["commands"].as_array().unwrap();
    let command_names: Vec<&str> = commands.iter().filter_map(|c| c["name"].as_str()).collect();
    assert!(
        command_names.contains(&"apply"),
        "commands must include apply"
    );
    assert!(
        command_names.contains(&"diff"),
        "commands must include diff"
    );
    assert!(
        command_names.contains(&"check"),
        "commands must include check"
    );

    let apply = commands.iter().find(|c| c["name"] == "apply").unwrap();
    assert_eq!(
        apply["mutating"], true,
        "apply command must be marked mutating"
    );

    let diff = commands.iter().find(|c| c["name"] == "diff").unwrap();
    assert_eq!(
        diff["mutating"], false,
        "diff command must be marked non-mutating"
    );
}

#[test]
fn error_envelope_on_stderr() {
    // Apply without a valid project dir or targets - the missing project will cause
    // a config/target error that gets emitted as a JSON envelope to stderr.
    // We pipe stdin from /dev/null to simulate non-TTY (triggers confirmation_required).
    let output = Command::new(env!("CARGO_BIN_EXE_verg"))
        .args([
            "apply",
            "--targets",
            "nonexistent",
            "--path",
            "/tmp/verg-no-such-dir",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(stderr.trim())
        .unwrap_or_else(|_| panic!("stderr must be a JSON envelope, got: {stderr}"));
    assert!(parsed["error"].is_object(), "error field must be an object");
    assert!(
        parsed["error"]["kind"].is_string(),
        "error.kind must be a string"
    );
    assert!(
        parsed["error"]["message"].is_string(),
        "error.message must be a string"
    );
}

#[test]
fn output_flag_json_works() {
    let output = Command::new(env!("CARGO_BIN_EXE_verg"))
        .args(["--output", "json", "schema"])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(parsed["name"], "verg");
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
