use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::error::Error;

use super::{ResolvedResource, ResourceResult, ResourceStatus};

const CRON_DIR: &str = "/etc/cron.d";
/// crond silently ignores files that are world-writable or have wrong permissions.
const CRON_MODE: u32 = 0o644;

struct CronJob {
    schedule: String,
    command: String,
}

/// Validate that a cron resource name is safe to use as an `/etc/cron.d/` filename.
/// Only `[a-zA-Z0-9_-]` is allowed to prevent path traversal.
fn validate_name(name: &str) -> Result<(), Error> {
    if name.is_empty() {
        return Err(Error::Resource("cron name cannot be empty".into()));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(Error::Resource(format!(
            "cron name '{name}' contains invalid characters (allowed: [a-zA-Z0-9_-])"
        )));
    }
    Ok(())
}

/// Validate a cron schedule expression: must be exactly 5 whitespace-separated
/// fields containing only `[0-9*/,-]`. Does not require a cron parse crate.
fn validate_schedule(schedule: &str) -> Result<(), Error> {
    if schedule.contains('\n') {
        return Err(Error::Resource(
            "cron schedule must not contain newlines".into(),
        ));
    }
    let fields: Vec<&str> = schedule.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(Error::Resource(format!(
            "cron schedule '{schedule}' must have exactly 5 fields (minute hour dom month weekday)"
        )));
    }
    let field_maxima = [59u32, 23, 31, 12, 7];
    for (field, &max) in fields.iter().zip(field_maxima.iter()) {
        if !field
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '*' | '/' | '-' | ','))
        {
            return Err(Error::Resource(format!(
                "cron schedule field '{field}' contains invalid characters"
            )));
        }
        // Check that any numeric values are within the allowed range.
        for segment in field.split([',', '-', '/']) {
            if let Ok(n) = segment.parse::<u32>()
                && n > max
            {
                return Err(Error::Resource(format!(
                    "cron schedule field '{field}' value {n} exceeds maximum {max}"
                )));
            }
        }
    }
    Ok(())
}

/// Reject newlines in commands to prevent injecting extra cron lines.
fn validate_command(command: &str) -> Result<(), Error> {
    if command.contains('\n') {
        return Err(Error::Resource(
            "cron command must not contain newlines (use a script file for multi-line commands)"
                .into(),
        ));
    }
    Ok(())
}

/// Parse the `jobs` array (multi-job form) or `schedule`+`command` (single-job form).
/// Returns an error if both forms are provided simultaneously.
fn parse_jobs(resource: &ResolvedResource) -> Result<Vec<CronJob>, Error> {
    let has_jobs = resource.props.contains_key("jobs");
    let has_schedule = resource.props.contains_key("schedule");
    let has_command = resource.props.contains_key("command");

    if has_jobs && (has_schedule || has_command) {
        return Err(Error::Resource(
            "cron resource must use either 'jobs' (multi-job) or \
             'schedule'+'command' (single-job), not both"
                .into(),
        ));
    }

    if has_jobs {
        let arr = resource
            .props
            .get("jobs")
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error::Resource("cron 'jobs' must be an array of tables".into()))?;

        if arr.is_empty() {
            return Err(Error::Resource(
                "cron 'jobs' array must not be empty".into(),
            ));
        }

        let mut jobs = Vec::with_capacity(arr.len());
        for (i, item) in arr.iter().enumerate() {
            let table = item
                .as_table()
                .ok_or_else(|| Error::Resource(format!("cron 'jobs[{i}]' must be a table")))?;
            let schedule = table
                .get("schedule")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::Resource(format!("cron 'jobs[{i}]' requires 'schedule'")))?
                .to_string();
            let command = table
                .get("command")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::Resource(format!("cron 'jobs[{i}]' requires 'command'")))?
                .to_string();
            validate_schedule(&schedule)?;
            validate_command(&command)?;
            jobs.push(CronJob { schedule, command });
        }
        return Ok(jobs);
    }

    if has_schedule || has_command {
        let schedule = resource
            .props
            .get("schedule")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                Error::Resource(
                    "cron single-job form requires both 'schedule' and 'command'".into(),
                )
            })?
            .to_string();
        let command = resource
            .props
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                Error::Resource(
                    "cron single-job form requires both 'schedule' and 'command'".into(),
                )
            })?
            .to_string();
        validate_schedule(&schedule)?;
        validate_command(&command)?;
        return Ok(vec![CronJob { schedule, command }]);
    }

    Err(Error::Resource(
        "cron resource requires 'jobs' (multi-job) or 'schedule'+'command' (single-job)".into(),
    ))
}

fn build_file_content(
    user: &str,
    jobs: &[CronJob],
    mailto: Option<&str>,
    env_table: Option<&toml::value::Table>,
) -> String {
    let mut content = String::from("# Managed by verg — do not edit manually\n");

    if let Some(m) = mailto {
        content.push_str(&format!("MAILTO={m}\n"));
    }
    if let Some(env) = env_table {
        let mut pairs: Vec<_> = env.iter().collect();
        pairs.sort_by_key(|(k, _)| k.as_str());
        for (k, v) in pairs {
            if let Some(val) = v.as_str() {
                content.push_str(&format!("{k}={val}\n"));
            }
        }
    }
    if mailto.is_some() || env_table.is_some_and(|e| !e.is_empty()) {
        content.push('\n');
    }

    for job in jobs {
        content.push_str(&format!("{}  {}  {}\n", job.schedule, user, job.command));
    }

    content
}

pub fn execute(resource: &ResolvedResource, dry_run: bool) -> Result<ResourceResult, Error> {
    validate_name(&resource.name)?;

    let state = resource
        .props
        .get("state")
        .and_then(|v| v.as_str())
        .unwrap_or("present");

    let cron_path = format!("{CRON_DIR}/{}", resource.name);
    let target = Path::new(&cron_path);

    if state == "absent" {
        if !target.exists() {
            return Ok(ResourceResult {
                resource_type: "cron".into(),
                name: resource.name.clone(),
                status: ResourceStatus::Ok,
                diff: None,
                from: None,
                to: None,
                error: None,
                output: None,
            });
        }
        let current = std::fs::read_to_string(target).ok();
        if dry_run {
            return Ok(ResourceResult {
                resource_type: "cron".into(),
                name: resource.name.clone(),
                status: ResourceStatus::Changed,
                diff: Some(format!("would remove {cron_path}")),
                from: current,
                to: None,
                error: None,
                output: None,
            });
        }
        std::fs::remove_file(target)
            .map_err(|e| Error::Resource(format!("failed to remove {cron_path}: {e}")))?;
        return Ok(ResourceResult {
            resource_type: "cron".into(),
            name: resource.name.clone(),
            status: ResourceStatus::Changed,
            diff: Some(format!("removed {cron_path}")),
            from: current,
            to: None,
            error: None,
            output: None,
        });
    }

    let user = resource
        .props
        .get("user")
        .and_then(|v| v.as_str())
        .ok_or_else(|| Error::Resource("cron resource requires 'user'".into()))?;

    let jobs = parse_jobs(resource)?;
    let mailto = resource.props.get("mailto").and_then(|v| v.as_str());
    let env_table = resource.props.get("env").and_then(|v| v.as_table());
    let desired = build_file_content(user, &jobs, mailto, env_table);

    let current = if target.exists() {
        Some(
            std::fs::read_to_string(target)
                .map_err(|e| Error::Resource(format!("failed to read {cron_path}: {e}")))?,
        )
    } else {
        None
    };

    let mode_ok = if target.exists() {
        let mode = std::fs::metadata(target)
            .map_err(|e| Error::Resource(format!("failed to stat {cron_path}: {e}")))?
            .permissions()
            .mode()
            & 0o7777;
        mode == CRON_MODE
    } else {
        false
    };

    let content_ok = current.as_deref() == Some(desired.as_str());

    if content_ok && mode_ok {
        return Ok(ResourceResult {
            resource_type: "cron".into(),
            name: resource.name.clone(),
            status: ResourceStatus::Ok,
            diff: None,
            from: None,
            to: None,
            error: None,
            output: None,
        });
    }

    if dry_run {
        let diff = if !content_ok {
            format!("would write {cron_path}")
        } else {
            format!("would fix mode on {cron_path}")
        };
        return Ok(ResourceResult {
            resource_type: "cron".into(),
            name: resource.name.clone(),
            status: ResourceStatus::Changed,
            diff: Some(diff),
            from: current,
            to: Some(desired),
            error: None,
            output: None,
        });
    }

    if !content_ok {
        std::fs::write(target, &desired)
            .map_err(|e| Error::Resource(format!("failed to write {cron_path}: {e}")))?;
    }
    // Always enforce 0644 — crond silently ignores world-writable files
    std::fs::set_permissions(target, std::fs::Permissions::from_mode(CRON_MODE))
        .map_err(|e| Error::Resource(format!("failed to chmod {cron_path}: {e}")))?;

    let diff = if !content_ok {
        format!("wrote {cron_path}")
    } else {
        format!("fixed mode on {cron_path}")
    };

    Ok(ResourceResult {
        resource_type: "cron".into(),
        name: resource.name.clone(),
        status: ResourceStatus::Changed,
        diff: Some(diff),
        from: current,
        to: Some(desired),
        error: None,
        output: None,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::resources::ResolvedResource;

    fn make_resource(name: &str, props: HashMap<String, toml::Value>) -> ResolvedResource {
        ResolvedResource {
            resource_type: "cron".into(),
            name: name.into(),
            props,
            after: vec![],
            notify: vec![],
            when: None,
            handler: false,
            register: None,
        }
    }

    #[test]
    fn name_rejects_path_traversal() {
        assert!(validate_name("../etc/passwd").is_err());
        assert!(validate_name("my job").is_err());
        assert!(validate_name("a/b").is_err());
        assert!(validate_name("").is_err());
    }

    #[test]
    fn name_accepts_valid_identifiers() {
        assert!(validate_name("hours-automation").is_ok());
        assert!(validate_name("gphotos_sync").is_ok());
        assert!(validate_name("backup123").is_ok());
    }

    #[test]
    fn schedule_rejects_wrong_field_count() {
        assert!(validate_schedule("0 20 *").is_err());
        assert!(validate_schedule("0 20 * * * *").is_err());
    }

    #[test]
    fn schedule_rejects_invalid_characters() {
        assert!(validate_schedule("0 20 * * $HOME").is_err());
        assert!(validate_schedule("0 20\n* * *").is_err());
    }

    #[test]
    fn schedule_rejects_out_of_range_values() {
        assert!(validate_schedule("60 20 * * *").is_err()); // minute > 59
        assert!(validate_schedule("0 25 * * *").is_err()); // hour > 23
    }

    #[test]
    fn schedule_accepts_valid_expressions() {
        assert!(validate_schedule("0 3 * * *").is_ok());
        assert!(validate_schedule("0 20 * * 1-4").is_ok());
        assert!(validate_schedule("*/15 * * * *").is_ok());
        assert!(validate_schedule("0 0 1,15 * *").is_ok());
        assert!(validate_schedule("0 20 * * 5").is_ok());
    }

    #[test]
    fn command_rejects_newlines() {
        assert!(validate_command("echo foo\necho bar").is_err());
    }

    #[test]
    fn command_accepts_normal_commands() {
        assert!(validate_command("/haven/hours-automation/run.sh --close-week").is_ok());
    }

    #[test]
    fn parse_jobs_rejects_both_forms_simultaneously() {
        let mut props = HashMap::new();
        props.insert("user".into(), toml::Value::String("root".into()));
        props.insert("schedule".into(), toml::Value::String("0 3 * * *".into()));
        props.insert("command".into(), toml::Value::String("/backup.sh".into()));
        props.insert("jobs".into(), toml::Value::Array(vec![]));
        let r = make_resource("test", props);
        assert!(parse_jobs(&r).is_err());
    }

    #[test]
    fn parse_jobs_single_form() {
        let mut props = HashMap::new();
        props.insert("user".into(), toml::Value::String("root".into()));
        props.insert("schedule".into(), toml::Value::String("0 3 * * *".into()));
        props.insert(
            "command".into(),
            toml::Value::String("/root/backup.sh".into()),
        );
        let r = make_resource("test", props);
        let jobs = parse_jobs(&r).unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].schedule, "0 3 * * *");
        assert_eq!(jobs[0].command, "/root/backup.sh");
    }

    #[test]
    fn parse_jobs_multi_form() {
        let job1 = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert(
                "schedule".into(),
                toml::Value::String("0 20 * * 1-4".into()),
            );
            t.insert("command".into(), toml::Value::String("/run.sh".into()));
            t
        });
        let job2 = toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert("schedule".into(), toml::Value::String("0 20 * * 5".into()));
            t.insert(
                "command".into(),
                toml::Value::String("/run.sh --close-week".into()),
            );
            t
        });
        let mut props = HashMap::new();
        props.insert("user".into(), toml::Value::String("root".into()));
        props.insert("jobs".into(), toml::Value::Array(vec![job1, job2]));
        let r = make_resource("hours", props);
        let jobs = parse_jobs(&r).unwrap();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[1].command, "/run.sh --close-week");
    }

    #[test]
    fn parse_jobs_rejects_empty_jobs_array() {
        let mut props = HashMap::new();
        props.insert("user".into(), toml::Value::String("root".into()));
        props.insert("jobs".into(), toml::Value::Array(vec![]));
        let r = make_resource("test", props);
        assert!(parse_jobs(&r).is_err());
    }

    #[test]
    fn file_content_single_job() {
        let jobs = vec![CronJob {
            schedule: "0 3 * * *".into(),
            command: "/root/backup.sh".into(),
        }];
        let content = build_file_content("root", &jobs, None, None);
        assert!(content.starts_with("# Managed by verg"));
        assert!(content.contains("0 3 * * *  root  /root/backup.sh\n"));
    }

    #[test]
    fn file_content_with_mailto() {
        let jobs = vec![CronJob {
            schedule: "0 3 * * *".into(),
            command: "/root/backup.sh".into(),
        }];
        let content = build_file_content("root", &jobs, Some(""), None);
        assert!(content.contains("MAILTO=\n"));
        // Blank line separates env from job lines
        assert!(content.contains("MAILTO=\n\n"));
    }

    #[test]
    fn file_content_multi_job() {
        let jobs = vec![
            CronJob {
                schedule: "0 20 * * 1-4".into(),
                command: "/run.sh".into(),
            },
            CronJob {
                schedule: "0 20 * * 5".into(),
                command: "/run.sh --close-week".into(),
            },
        ];
        let content = build_file_content("root", &jobs, None, None);
        assert!(content.contains("0 20 * * 1-4  root  /run.sh\n"));
        assert!(content.contains("0 20 * * 5  root  /run.sh --close-week\n"));
    }
}
