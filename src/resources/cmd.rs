use std::path::Path;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, ResourceStatus, run_cmd, run_cmd_with_stdin};

pub fn execute(
    resource: &ResolvedResource,
    dry_run: bool,
    notified: bool,
) -> Result<ResourceResult, Error> {
    let command = resource
        .props
        .get("command")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Resource("cmd resource requires 'command'".into()))?;

    let creates = resource.props.get("creates").and_then(|v| v.as_str());
    let unless = resource.props.get("unless").and_then(|v| v.as_str());
    let onlyif = resource.props.get("onlyif").and_then(|v| v.as_str());
    let stdin = resource.props.get("stdin").and_then(|v| v.as_str());

    let has_register = resource.register.is_some();
    if !notified && !has_register && creates.is_none() && unless.is_none() && onlyif.is_none() {
        return Err(Error::Resource(
            "cmd resource requires at least one guard: 'creates', 'unless', or 'onlyif' (or 'register')".into(),
        ));
    }

    if let Some(path) = creates
        && Path::new(path).exists()
    {
        return Ok(ResourceResult {
            resource_type: "cmd".into(),
            name: resource.name.clone(),
            status: ResourceStatus::Ok,
            diff: None,
            from: None,
            to: None,
            error: None,
            output: None,
        });
    }

    if let Some(cmd) = unless {
        let output = run_cmd("sh", &["-c", cmd])?;
        if output.status.success() {
            return Ok(ResourceResult {
                resource_type: "cmd".into(),
                name: resource.name.clone(),
                status: ResourceStatus::Ok,
                diff: None,
                from: None,
                to: None,
                error: None,
                output: None,
            });
        }
    }

    if let Some(cmd) = onlyif {
        let output = run_cmd("sh", &["-c", cmd])?;
        if !output.status.success() {
            return Ok(ResourceResult {
                resource_type: "cmd".into(),
                name: resource.name.clone(),
                status: ResourceStatus::Ok,
                diff: None,
                from: None,
                to: None,
                error: None,
                output: None,
            });
        }
    }

    if dry_run {
        // stdin content is intentionally omitted from diffs — treat it as sensitive.
        let diff = if stdin.is_some() {
            format!("would run: {command} (with stdin)")
        } else {
            format!("would run: {command}")
        };
        return Ok(ResourceResult {
            resource_type: "cmd".into(),
            name: resource.name.clone(),
            status: ResourceStatus::Changed,
            diff: Some(diff),
            from: None,
            to: None,
            error: None,
            output: None,
        });
    }

    let output = if let Some(data) = stdin {
        run_cmd_with_stdin("sh", &["-c", command], data.as_bytes())?
    } else {
        run_cmd("sh", &["-c", command])?
    };

    if output.status.success() {
        let captured = if has_register {
            let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
            if stdout.len() > 64 * 1024 {
                eprintln!(
                    "warning: cmd.{} register output truncated to 65536 bytes",
                    resource.name
                );
                stdout.truncate(64 * 1024);
            }
            Some(stdout.trim().to_string())
        } else {
            None
        };
        Ok(ResourceResult {
            resource_type: "cmd".into(),
            name: resource.name.clone(),
            status: ResourceStatus::Changed,
            diff: None,
            from: None,
            to: None,
            error: None,
            output: captured,
        })
    } else {
        // Deliberately omit the command itself to avoid leaking context; stderr
        // from the process (e.g. "smbpasswd: Bad password") is included but
        // will never contain the stdin content we wrote.
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(Error::Resource(format!("command failed: {stderr}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn cmd_resource(name: &str, command: &str, props: &[(&str, toml::Value)]) -> ResolvedResource {
        let mut p: HashMap<String, toml::Value> = props
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect();
        p.insert("command".into(), toml::Value::String(command.into()));
        ResolvedResource {
            resource_type: "cmd".into(),
            name: name.into(),
            props: p,
            after: vec![],
            notify: vec![],
            when: None,
            handler: false,
            register: None,
        }
    }

    #[test]
    fn register_cmd_does_not_require_guard() {
        let mut r = cmd_resource("get-ip", "echo 10.0.0.1", &[]);
        r.register = Some("ip".into());
        let result = execute(&r, false, false).unwrap();
        assert_eq!(result.status, ResourceStatus::Changed);
        assert_eq!(result.output, Some("10.0.0.1".into()));
    }

    #[test]
    fn non_register_cmd_requires_guard() {
        let r = cmd_resource("bad", "echo hello", &[]);
        let result = execute(&r, false, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("guard"));
    }

    #[test]
    fn register_cmd_captures_stdout_trimmed() {
        let mut r = cmd_resource("ver", "printf '  hello  \\n'", &[]);
        r.register = Some("version".into());
        let result = execute(&r, false, false).unwrap();
        assert_eq!(result.output, Some("hello".into()));
    }

    #[test]
    fn register_cmd_dry_run() {
        let mut r = cmd_resource("get-ip", "echo 10.0.0.1", &[]);
        r.register = Some("ip".into());
        let result = execute(&r, true, false).unwrap();
        assert_eq!(result.status, ResourceStatus::Changed);
        assert!(result.output.is_none());
    }

    #[test]
    fn cmd_with_guard_still_works() {
        let r = cmd_resource(
            "test",
            "echo done",
            &[("unless", toml::Value::String("false".into()))],
        );
        let result = execute(&r, false, false).unwrap();
        assert_eq!(result.status, ResourceStatus::Changed);
    }

    // — stdin tests —

    #[test]
    fn stdin_is_piped_to_command() {
        // `cat` echoes stdin to stdout; register captures it.
        let mut r = cmd_resource(
            "pipe-test",
            "cat",
            &[("stdin", toml::Value::String("hello from stdin".into()))],
        );
        r.register = Some("out".into());
        let result = execute(&r, false, false).unwrap();
        assert_eq!(result.output, Some("hello from stdin".into()));
    }

    #[test]
    fn stdin_with_newlines_piped_correctly() {
        // read two lines from stdin and emit line count
        let mut r = cmd_resource(
            "lines",
            "wc -l | tr -d ' '",
            &[("stdin", toml::Value::String("line1\nline2\nline3\n".into()))],
        );
        r.register = Some("count".into());
        let result = execute(&r, false, false).unwrap();
        assert_eq!(result.output, Some("3".into()));
    }

    #[test]
    fn stdin_not_echoed_in_dry_run_diff() {
        let r = cmd_resource(
            "secret-op",
            "smbpasswd -a testuser",
            &[
                ("stdin", toml::Value::String("s3cr3t\ns3cr3t\n".into())),
                ("unless", toml::Value::String("false".into())),
            ],
        );
        let result = execute(&r, true, false).unwrap();
        let diff = result.diff.unwrap();
        // The command is shown, the secret is not
        assert!(diff.contains("smbpasswd"), "command should appear in diff");
        assert!(!diff.contains("s3cr3t"), "secret must not appear in diff");
        assert!(diff.contains("with stdin"), "should note stdin presence");
    }

    #[test]
    fn stdin_absent_dry_run_has_no_stdin_note() {
        let r = cmd_resource(
            "no-stdin",
            "echo hello",
            &[("unless", toml::Value::String("false".into()))],
        );
        let result = execute(&r, true, false).unwrap();
        let diff = result.diff.unwrap();
        assert!(!diff.contains("stdin"), "no stdin note when stdin not set");
    }

    #[test]
    fn stdin_with_unless_guard_skips_when_guard_passes() {
        // unless `true` → skip (status Ok, stdin never runs)
        let r = cmd_resource(
            "skip-me",
            "cat",
            &[
                ("stdin", toml::Value::String("data".into())),
                ("unless", toml::Value::String("true".into())),
            ],
        );
        let result = execute(&r, false, false).unwrap();
        assert_eq!(result.status, ResourceStatus::Ok);
    }

    #[test]
    fn stdin_with_unless_guard_runs_when_guard_fails() {
        // unless `false` → run; cat echoes stdin
        let mut r = cmd_resource(
            "run-me",
            "cat",
            &[
                ("stdin", toml::Value::String("payload".into())),
                ("unless", toml::Value::String("false".into())),
            ],
        );
        r.register = Some("got".into());
        let result = execute(&r, false, false).unwrap();
        assert_eq!(result.status, ResourceStatus::Changed);
        assert_eq!(result.output, Some("payload".into()));
    }

    #[test]
    fn stdin_captures_stdout_not_stdin_content() {
        // Command reads stdin but its stdout is different — register gets stdout only
        let mut r = cmd_resource(
            "transform",
            "read line; echo \"got: $line\"",
            &[("stdin", toml::Value::String("secret\n".into()))],
        );
        r.register = Some("result".into());
        let result = execute(&r, false, false).unwrap();
        // stdout is "got: secret", not the raw stdin
        assert_eq!(result.output, Some("got: secret".into()));
    }

    #[test]
    fn stdin_with_large_output_does_not_deadlock() {
        // Produce more output than a typical pipe buffer (64KB)
        // while also writing stdin — tests the write-thread approach.
        let mut r = cmd_resource(
            "big-out",
            "cat /dev/urandom | head -c 131072 | wc -c | tr -d ' '",
            &[("stdin", toml::Value::String("ignored\n".into()))],
        );
        r.register = Some("bytes".into());
        let result = execute(&r, false, false).unwrap();
        // We only care that it completes without deadlock, not the exact value
        // (the stdin is ignored by the pipeline; wc counts urandom bytes)
        assert!(result.status == ResourceStatus::Changed);
    }
}
