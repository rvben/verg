use std::path::Path;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, ResourceStatus, run_cmd};

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
        return Ok(ResourceResult {
            resource_type: "cmd".into(),
            name: resource.name.clone(),
            status: ResourceStatus::Changed,
            diff: Some(format!("would run: {command}")),
            from: None,
            to: None,
            error: None,
            output: None,
        });
    }

    let output = run_cmd("sh", &["-c", command])?;
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
}
