use std::collections::HashMap;

use crate::error::Error;

pub fn interpolate(template: &str, vars: &HashMap<String, toml::Value>) -> Result<String, Error> {
    let mut result = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '{' && chars.peek() == Some(&'{') {
            chars.next();
            let mut var_name = String::new();
            loop {
                match chars.next() {
                    Some('}') if chars.peek() == Some(&'}') => {
                        chars.next();
                        break;
                    }
                    Some(c) => var_name.push(c),
                    None => return Err(Error::Parse("unclosed {{ in template".into())),
                }
            }
            let var_name = var_name.trim();
            let value = vars
                .get(var_name)
                .ok_or_else(|| Error::Parse(format!("undefined variable: {var_name}")))?;
            match value {
                toml::Value::String(s) => result.push_str(s),
                toml::Value::Integer(i) => result.push_str(&i.to_string()),
                toml::Value::Float(f) => result.push_str(&f.to_string()),
                toml::Value::Boolean(b) => result.push_str(&b.to_string()),
                other => result.push_str(&other.to_string()),
            }
        } else {
            result.push(c);
        }
    }

    Ok(result)
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
        assert_eq!(interpolate("pkg: {{ name }}", &v).unwrap(), "pkg: nginx");
    }

    #[test]
    fn integer_substitution() {
        let v = vars(&[("port", toml::Value::Integer(8080))]);
        assert_eq!(interpolate("listen {{ port }}", &v).unwrap(), "listen 8080");
    }

    #[test]
    fn multiple_vars() {
        let v = vars(&[
            ("host", toml::Value::String("localhost".into())),
            ("port", toml::Value::Integer(3000)),
        ]);
        assert_eq!(
            interpolate("{{ host }}:{{ port }}", &v).unwrap(),
            "localhost:3000"
        );
    }

    #[test]
    fn no_vars_passthrough() {
        let v = HashMap::new();
        assert_eq!(
            interpolate("no variables here", &v).unwrap(),
            "no variables here"
        );
    }

    #[test]
    fn undefined_var_errors() {
        let v = HashMap::new();
        let result = interpolate("{{ missing }}", &v);
        assert!(matches!(result, Err(Error::Parse(_))));
    }

    #[test]
    fn unclosed_brace_errors() {
        let v = HashMap::new();
        let result = interpolate("{{ unclosed", &v);
        assert!(matches!(result, Err(Error::Parse(_))));
    }

    #[test]
    fn whitespace_tolerance() {
        let v = vars(&[("x", toml::Value::String("val".into()))]);
        assert_eq!(interpolate("{{x}}", &v).unwrap(), "val");
        assert_eq!(interpolate("{{  x  }}", &v).unwrap(), "val");
    }
}
