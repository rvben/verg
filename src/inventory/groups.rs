use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

use crate::error::Error;

#[derive(Debug, Clone, Deserialize)]
pub struct GroupDef {
    #[serde(default)]
    pub vars: HashMap<String, toml::Value>,
}

pub fn load_groups(dir: &Path) -> Result<HashMap<String, GroupDef>, Error> {
    let mut groups = HashMap::new();
    if !dir.exists() {
        return Ok(groups);
    }
    let entries = std::fs::read_dir(dir)
        .map_err(|e| Error::Config(format!("failed to read {}: {e}", dir.display())))?;
    for entry in entries {
        let entry = entry.map_err(|e| Error::Config(e.to_string()))?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "toml") {
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            let content = std::fs::read_to_string(&path)
                .map_err(|e| Error::Config(format!("failed to read {}: {e}", path.display())))?;
            let group: GroupDef = toml::from_str(&content)
                .map_err(|e| Error::Parse(format!("failed to parse {}: {e}", path.display())))?;
            groups.insert(name, group);
        }
    }
    Ok(groups)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::TempDir;

    use super::*;

    #[test]
    fn load_group_vars() {
        let dir = TempDir::new().unwrap();
        let mut f = std::fs::File::create(dir.path().join("web.toml")).unwrap();
        write!(
            f,
            r#"
[vars]
http_port = 80
document_root = "/var/www/html"
"#
        )
        .unwrap();

        let groups = load_groups(dir.path()).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups["web"].vars["http_port"], toml::Value::Integer(80));
    }

    #[test]
    fn missing_groups_dir_returns_empty() {
        let groups = load_groups(Path::new("/nonexistent/groups")).unwrap();
        assert!(groups.is_empty());
    }

    #[test]
    fn ignores_non_toml_files() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("readme.md"), "# groups").unwrap();

        let groups = load_groups(dir.path()).unwrap();
        assert!(groups.is_empty());
    }
}
