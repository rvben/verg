use crate::error::Error;

/// Loads and decrypts secrets from `base_dir/secrets.age`.
///
/// Returns an empty JSON object when the file does not exist (secrets are optional).
/// The decrypted file must be a valid TOML document; it is converted to a JSON object.
pub fn load_secrets(
    base_dir: &std::path::Path,
    identity: Option<&std::path::Path>,
) -> Result<serde_json::Value, Error> {
    let secrets_file = base_dir.join("secrets.age");

    if !secrets_file.exists() {
        return Ok(serde_json::Value::Object(serde_json::Map::new()));
    }

    let identity = identity.ok_or_else(|| {
        Error::Config(
            "verg/secrets.age exists but no age identity was provided \
             (set --age-identity or VERG_AGE_IDENTITY)"
                .into(),
        )
    })?;

    let output = std::process::Command::new("age")
        .arg("--decrypt")
        .arg("--identity")
        .arg(identity)
        .arg(&secrets_file)
        .output()
        .map_err(|e| {
            Error::Config(format!(
                "failed to run age (is the age CLI installed on the control host?): {e}"
            ))
        })?;

    if !output.status.success() {
        return Err(Error::Config(format!(
            "age decrypt failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }

    let plaintext = String::from_utf8(output.stdout)
        .map_err(|e| Error::Parse(format!("decrypted secrets are not valid UTF-8: {e}")))?;

    let parsed = toml::from_str::<toml::Value>(&plaintext)
        .map_err(|e| Error::Parse(format!("decrypted secrets are not valid TOML: {e}")))?;

    serde_json::to_value(parsed)
        .map_err(|e| Error::Config(format!("failed to convert secrets to JSON: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::process::Command;

    #[test]
    fn no_secrets_file_returns_empty_object() {
        let dir = tempfile::tempdir().expect("tempdir");
        let result = load_secrets(dir.path(), None).expect("should succeed");
        let obj = result.as_object().expect("should be object");
        assert!(obj.is_empty(), "expected empty object, got: {obj:?}");
    }

    #[test]
    fn secrets_file_present_without_identity_is_config_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("secrets.age"), b"anything").expect("write");
        let err = load_secrets(dir.path(), None).expect_err("should fail");
        match &err {
            Error::Config(msg) => assert!(
                msg.contains("identity"),
                "error message should mention 'identity', got: {msg}"
            ),
            other => panic!("expected Error::Config, got: {other:?}"),
        }
    }

    #[test]
    fn round_trip_age_decrypt_and_toml_to_json() {
        // Skip if age or age-keygen are not available on this machine.
        if Command::new("age-keygen")
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }
        if Command::new("age").arg("--version").output().is_err() {
            return;
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let key_path = dir.path().join("key.txt");
        let toml_path = dir.path().join("secrets.toml");
        let encrypted_path = dir.path().join("secrets.age");

        // Generate an age identity (private key).
        let keygen = Command::new("age-keygen")
            .arg("-o")
            .arg(&key_path)
            .output()
            .expect("age-keygen");
        assert!(keygen.status.success(), "age-keygen failed");

        // Derive the recipient (public key) from the identity file.
        let recipient_out = Command::new("age-keygen")
            .arg("-y")
            .arg(&key_path)
            .output()
            .expect("age-keygen -y");
        assert!(recipient_out.status.success(), "age-keygen -y failed");
        let recipient = String::from_utf8(recipient_out.stdout)
            .expect("recipient utf8")
            .trim()
            .to_string();

        // Write a TOML secrets file with scalars and a nested table.
        let toml_content = "app_token = \"s3cr3t\"\nport = 5432\n\n[db]\nname = \"mydb\"\n";
        fs::write(&toml_path, toml_content).expect("write toml");

        // Encrypt the TOML file to the recipient.
        let encrypt = Command::new("age")
            .arg("--encrypt")
            .arg("-r")
            .arg(&recipient)
            .arg("-o")
            .arg(&encrypted_path)
            .arg(&toml_path)
            .output()
            .expect("age encrypt");
        assert!(
            encrypt.status.success(),
            "age encrypt failed: {}",
            String::from_utf8_lossy(&encrypt.stderr)
        );

        // Verify load_secrets decrypts and parses correctly.
        let value = load_secrets(dir.path(), Some(&key_path)).expect("load_secrets");

        let obj = value.as_object().expect("should be json object");

        assert_eq!(
            obj.get("app_token").and_then(|v| v.as_str()),
            Some("s3cr3t"),
            "app_token mismatch"
        );
        assert_eq!(
            obj.get("port").and_then(|v| v.as_i64()),
            Some(5432),
            "port mismatch"
        );

        // Nested table: [db] -> { "name": "mydb" }
        let db = obj
            .get("db")
            .and_then(|v| v.as_object())
            .expect("db should be a nested object");
        assert_eq!(
            db.get("name").and_then(|v| v.as_str()),
            Some("mydb"),
            "db.name mismatch"
        );
    }
}
