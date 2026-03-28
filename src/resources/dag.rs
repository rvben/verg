use std::collections::{HashMap, VecDeque};

use crate::error::Error;

use super::ResolvedResource;

pub fn resolve_order(resources: &[ResolvedResource]) -> Result<Vec<Vec<&ResolvedResource>>, Error> {
    let by_fqn: HashMap<String, &ResolvedResource> =
        resources.iter().map(|r| (r.fqn(), r)).collect();

    for r in resources {
        for dep in &r.after {
            if !by_fqn.contains_key(dep) {
                return Err(Error::Config(format!(
                    "resource {} depends on unknown resource {dep}",
                    r.fqn()
                )));
            }
        }
    }

    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();

    for r in resources {
        let fqn = r.fqn();
        in_degree.entry(fqn.clone()).or_insert(0);
        for dep in &r.after {
            *in_degree.entry(fqn.clone()).or_insert(0) += 1;
            dependents.entry(dep.clone()).or_default().push(fqn.clone());
        }
    }

    let mut layers = Vec::new();
    let mut queue: VecDeque<String> = in_degree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(name, _)| name.clone())
        .collect();

    let mut processed = 0;

    while !queue.is_empty() {
        let layer_names: Vec<String> = queue.drain(..).collect();
        let mut layer: Vec<&ResolvedResource> =
            layer_names.iter().map(|name| by_fqn[name]).collect();
        layer.sort_by_key(|r| r.fqn());
        processed += layer.len();

        for name in &layer_names {
            if let Some(deps) = dependents.get(name) {
                for dep in deps {
                    let deg = in_degree.get_mut(dep).unwrap();
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(dep.clone());
                    }
                }
            }
        }

        layers.push(layer);
    }

    if processed != resources.len() {
        return Err(Error::Config(
            "circular dependency detected in resources".into(),
        ));
    }

    Ok(layers)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn res(rtype: &str, name: &str, after: &[&str]) -> ResolvedResource {
        ResolvedResource {
            resource_type: rtype.into(),
            name: name.into(),
            props: HashMap::new(),
            after: after.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn no_dependencies_single_layer() {
        let resources = vec![res("pkg", "curl", &[]), res("pkg", "htop", &[])];
        let layers = resolve_order(&resources).unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].len(), 2);
    }

    #[test]
    fn linear_chain() {
        let resources = vec![
            res("pkg", "nginx", &[]),
            res("file", "conf", &["pkg.nginx"]),
            res("service", "nginx", &["file.conf"]),
        ];
        let layers = resolve_order(&resources).unwrap();
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0][0].fqn(), "pkg.nginx");
        assert_eq!(layers[1][0].fqn(), "file.conf");
        assert_eq!(layers[2][0].fqn(), "service.nginx");
    }

    #[test]
    fn diamond_dependency() {
        let resources = vec![
            res("pkg", "base", &[]),
            res("file", "a", &["pkg.base"]),
            res("file", "b", &["pkg.base"]),
            res("service", "app", &["file.a", "file.b"]),
        ];
        let layers = resolve_order(&resources).unwrap();
        assert_eq!(layers.len(), 3);
        assert_eq!(layers[0].len(), 1);
        assert_eq!(layers[1].len(), 2);
        assert_eq!(layers[2].len(), 1);
    }

    #[test]
    fn circular_dependency_errors() {
        let resources = vec![res("file", "a", &["file.b"]), res("file", "b", &["file.a"])];
        let result = resolve_order(&resources);
        assert!(matches!(result, Err(Error::Config(msg)) if msg.contains("circular")));
    }

    #[test]
    fn unknown_dependency_errors() {
        let resources = vec![res("file", "a", &["pkg.missing"])];
        let result = resolve_order(&resources);
        assert!(matches!(result, Err(Error::Config(msg)) if msg.contains("unknown")));
    }
}
