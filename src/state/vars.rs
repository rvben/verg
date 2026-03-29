use std::collections::HashMap;

use crate::error::Error;

/// Convert `toml::Value` vars to `serde_json::Value`, resolving any `$env.VAR`
/// references in string values to the actual environment variable value.
fn resolve_env_vars(
    vars: &HashMap<String, toml::Value>,
) -> Result<HashMap<String, serde_json::Value>, Error> {
    let mut out = HashMap::new();
    for (key, value) in vars {
        let json_val = toml_to_json(value)?;
        out.insert(key.clone(), json_val);
    }
    Ok(out)
}

fn toml_to_json(value: &toml::Value) -> Result<serde_json::Value, Error> {
    match value {
        toml::Value::String(s) => {
            if let Some(env_var) = s.strip_prefix("$env.") {
                let env_val = std::env::var(env_var).map_err(|_| {
                    Error::Parse(format!("environment variable '{env_var}' not set"))
                })?;
                Ok(serde_json::Value::String(env_val))
            } else {
                Ok(serde_json::Value::String(s.clone()))
            }
        }
        toml::Value::Integer(i) => Ok(serde_json::Value::Number((*i).into())),
        toml::Value::Float(f) => Ok(serde_json::json!(*f)),
        toml::Value::Boolean(b) => Ok(serde_json::Value::Bool(*b)),
        toml::Value::Array(arr) => {
            let items: Result<Vec<_>, _> = arr.iter().map(toml_to_json).collect();
            Ok(serde_json::Value::Array(items?))
        }
        toml::Value::Table(tbl) => {
            let mut map = serde_json::Map::new();
            for (k, v) in tbl {
                map.insert(k.clone(), toml_to_json(v)?);
            }
            Ok(serde_json::Value::Object(map))
        }
        toml::Value::Datetime(dt) => Ok(serde_json::Value::String(dt.to_string())),
    }
}

/// Render a template string using minijinja with the given variables.
///
/// Variables are converted from `toml::Value` to `serde_json::Value` first,
/// with any `$env.VAR` string values resolved from the environment.
/// A custom `env("VAR_NAME")` function is available in templates for direct
/// environment variable access.
pub fn render(template: &str, vars: &HashMap<String, toml::Value>) -> Result<String, Error> {
    let context = resolve_env_vars(vars)?;

    let mut env = minijinja::Environment::new();
    env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);

    env.add_function("env", |name: String| -> Result<String, minijinja::Error> {
        std::env::var(&name).map_err(|_| {
            minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                format!("environment variable '{name}' not set"),
            )
        })
    });

    env.render_str(template, &context)
        .map_err(|e| Error::Parse(format!("template error: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, toml::Value)]) -> HashMap<String, toml::Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn simple_substitution() {
        let v = vars(&[("name", toml::Value::String("nginx".into()))]);
        assert_eq!(render("pkg: {{ name }}", &v).unwrap(), "pkg: nginx");
    }

    #[test]
    fn integer_substitution() {
        let v = vars(&[("port", toml::Value::Integer(8080))]);
        assert_eq!(render("listen {{ port }}", &v).unwrap(), "listen 8080");
    }

    #[test]
    fn multiple_vars() {
        let v = vars(&[
            ("host", toml::Value::String("localhost".into())),
            ("port", toml::Value::Integer(3000)),
        ]);
        assert_eq!(
            render("{{ host }}:{{ port }}", &v).unwrap(),
            "localhost:3000"
        );
    }

    #[test]
    fn no_vars_passthrough() {
        let v = HashMap::new();
        assert_eq!(
            render("no variables here", &v).unwrap(),
            "no variables here"
        );
    }

    #[test]
    fn undefined_var_errors() {
        let v = HashMap::new();
        let result = render("{{ missing }}", &v);
        assert!(result.is_err());
    }

    #[test]
    fn unclosed_brace_errors() {
        let v = HashMap::new();
        let result = render("{{ unclosed", &v);
        assert!(result.is_err());
    }

    #[test]
    fn env_var_resolution_in_vars() {
        unsafe { std::env::set_var("VERG_TEST_SECRET", "s3cret") };
        let v = vars(&[(
            "api_key",
            toml::Value::String("$env.VERG_TEST_SECRET".into()),
        )]);
        assert_eq!(render("key={{ api_key }}", &v).unwrap(), "key=s3cret");
        unsafe { std::env::remove_var("VERG_TEST_SECRET") };
    }

    #[test]
    fn env_function_in_template() {
        unsafe { std::env::set_var("VERG_TEST_DIRECT", "direct_val") };
        let v = HashMap::new();
        assert_eq!(
            render("val={{ env('VERG_TEST_DIRECT') }}", &v).unwrap(),
            "val=direct_val"
        );
        unsafe { std::env::remove_var("VERG_TEST_DIRECT") };
    }

    #[test]
    fn whitespace_tolerance() {
        let v = vars(&[("x", toml::Value::String("val".into()))]);
        assert_eq!(render("{{x}}", &v).unwrap(), "val");
        assert_eq!(render("{{  x  }}", &v).unwrap(), "val");
    }

    #[test]
    fn for_loop() {
        let v = vars(&[(
            "packages",
            toml::Value::Array(vec![
                toml::Value::String("nginx".into()),
                toml::Value::String("curl".into()),
            ]),
        )]);
        assert_eq!(
            render("{% for p in packages %}{{ p }} {% endfor %}", &v).unwrap(),
            "nginx curl "
        );
    }

    #[test]
    fn if_conditional() {
        let v = vars(&[("enabled", toml::Value::Boolean(true))]);
        assert_eq!(
            render("{% if enabled %}yes{% else %}no{% endif %}", &v).unwrap(),
            "yes"
        );
    }

    #[test]
    fn default_filter() {
        let v = HashMap::new();
        assert_eq!(
            render("{{ missing | default('fallback') }}", &v).unwrap(),
            "fallback"
        );
    }

    #[test]
    fn join_filter() {
        let v = vars(&[(
            "items",
            toml::Value::Array(vec![
                toml::Value::String("a".into()),
                toml::Value::String("b".into()),
                toml::Value::String("c".into()),
            ]),
        )]);
        assert_eq!(render("{{ items | join(', ') }}", &v).unwrap(), "a, b, c");
    }

    #[test]
    fn nested_object_access() {
        let mut grafana = toml::map::Map::new();
        grafana.insert("port".into(), toml::Value::Integer(3000));
        grafana.insert("host".into(), toml::Value::String("grafana.local".into()));
        let v = vars(&[("grafana", toml::Value::Table(grafana))]);
        assert_eq!(render("{{ grafana.port }}", &v).unwrap(), "3000");
    }

    #[test]
    fn env_prefix_embedded_not_resolved() {
        let v = vars(&[("note", toml::Value::String("use $env.FOO".into()))]);
        assert_eq!(render("{{ note }}", &v).unwrap(), "use $env.FOO");
    }

    #[test]
    fn env_var_missing_in_vars_errors() {
        let v = vars(&[(
            "x",
            toml::Value::String("$env.VERG_NONEXISTENT_12345".into()),
        )]);
        let result = render("{{ x }}", &v);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not set"));
    }

    #[test]
    fn raw_block_passthrough() {
        let v = HashMap::new();
        assert_eq!(
            render("{% raw %}{{ not_a_var }}{% endraw %}", &v).unwrap(),
            "{{ not_a_var }}"
        );
    }
}
