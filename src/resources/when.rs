use std::collections::HashMap;

/// Evaluate a `when` expression against a set of facts.
///
/// Supported syntax:
///   "fact.arch == 'x86_64'"
///   "fact.hostname != 'caddy'"
///   "group.caddy"
///   "!group.caddy"
///   "fact.os == 'Ubuntu' && group.docker"
///   "fact.os == 'Ubuntu' || group.docker"
///
/// Operator precedence (lowest to highest): `||` < `&&`. No parentheses support.
pub fn evaluate(expr: &str, facts: &HashMap<String, String>) -> bool {
    let expr = expr.trim();

    // Handle || (OR) first - it binds looser than &&.
    if expr.contains("||") {
        return expr.split("||").any(|part| evaluate(part, facts));
    }
    // Then && (AND).
    if expr.contains("&&") {
        return expr.split("&&").all(|part| evaluate(part, facts));
    }

    let expr = expr.trim();

    // Negation: !group.X or !fact.X
    if let Some(rest) = expr.strip_prefix('!') {
        return !evaluate(rest.trim(), facts);
    }

    // Equality: fact.X == 'val' or fact.X != 'val'
    if let Some((lhs, rhs)) = expr.split_once("!=") {
        let key = lhs.trim();
        let val = rhs.trim().trim_matches('\'').trim_matches('"');
        return facts.get(key).map(|v| v.as_str() != val).unwrap_or(false);
    }
    if let Some((lhs, rhs)) = expr.split_once("==") {
        let key = lhs.trim();
        let val = rhs.trim().trim_matches('\'').trim_matches('"');
        return facts.get(key).map(|v| v.as_str() == val).unwrap_or(false);
    }

    // Boolean truth: group.X or fact.X (truthy if key exists and is not empty/false)
    if let Some(val) = facts.get(expr) {
        return !val.is_empty() && val != "false" && val != "0";
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts() -> HashMap<String, String> {
        let mut f = HashMap::new();
        f.insert("fact.arch".into(), "x86_64".into());
        f.insert("fact.hostname".into(), "home".into());
        f.insert("fact.os".into(), "Ubuntu".into());
        f.insert("group.docker".into(), "true".into());
        f.insert("group.caddy".into(), "true".into());
        f
    }

    #[test]
    fn equality() {
        assert!(evaluate("fact.arch == 'x86_64'", &facts()));
        assert!(!evaluate("fact.arch == 'aarch64'", &facts()));
    }

    #[test]
    fn inequality() {
        assert!(evaluate("fact.arch != 'aarch64'", &facts()));
        assert!(!evaluate("fact.arch != 'x86_64'", &facts()));
    }

    #[test]
    fn group_membership() {
        assert!(evaluate("group.docker", &facts()));
        assert!(!evaluate("group.monitoring", &facts()));
    }

    #[test]
    fn negation() {
        assert!(!evaluate("!group.docker", &facts()));
        assert!(evaluate("!group.monitoring", &facts()));
    }

    #[test]
    fn and_expression() {
        assert!(evaluate("fact.os == 'Ubuntu' && group.docker", &facts()));
        assert!(!evaluate("fact.os == 'Debian' && group.docker", &facts()));
    }

    #[test]
    fn or_expression() {
        assert!(evaluate("fact.os == 'Debian' || group.docker", &facts()));
        assert!(!evaluate(
            "fact.os == 'Debian' || group.monitoring",
            &facts()
        ));
    }

    #[test]
    fn missing_fact_is_false() {
        assert!(!evaluate("fact.nonexistent == 'val'", &facts()));
        assert!(!evaluate("group.nonexistent", &facts()));
    }

    #[test]
    fn missing_fact_inequality_is_false() {
        // A `!=` against an absent fact must NOT run the resource. An
        // indeterminate condition is treated as "skip", matching `==`.
        assert!(!evaluate("fact.nonexistent != 'val'", &facts()));
        // Symmetric with a misspelled key.
        assert!(!evaluate("fact.osss != 'Ubuntu'", &facts()));
    }

    #[test]
    fn or_binds_looser_than_and() {
        // fact.os == 'Ubuntu' is true; group.monitoring is false.
        // "true || false && false" must be true (|| has lower precedence).
        assert!(evaluate(
            "fact.os == 'Ubuntu' || group.monitoring && group.nonexistent",
            &facts()
        ));
        // "false && false || true" must be true.
        assert!(evaluate(
            "group.monitoring && group.nonexistent || fact.os == 'Ubuntu'",
            &facts()
        ));
    }
}
