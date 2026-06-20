//! Expected SHA-256 checksums for the verg-agent binaries of this version,
//! embedded at build time. Empty for local cargo (dev) builds, where checksum
//! verification is skipped.

use std::collections::HashMap;

const EMBEDDED_MANIFEST: &str = include_str!(concat!(env!("OUT_DIR"), "/agent_checksums.txt"));

/// Parse a `sha256  verg-agent-<target>` manifest into target -> hash.
fn parse_manifest(manifest: &str) -> HashMap<&str, &str> {
    let mut map = HashMap::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.split_whitespace();
        if let (Some(hash), Some(file)) = (parts.next(), parts.next())
            && let Some(target) = file.strip_prefix("verg-agent-")
        {
            map.insert(target, hash);
        }
    }
    map
}

/// Expected hash for an arch target triple, or None if not embedded.
pub fn expected_sha256(arch_target: &str) -> Option<&'static str> {
    parse_manifest(EMBEDDED_MANIFEST).get(arch_target).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manifest_lines() {
        let manifest = "\
abc123  verg-agent-x86_64-unknown-linux-gnu
def456  verg-agent-aarch64-unknown-linux-gnu
";
        let map = parse_manifest(manifest);
        assert_eq!(map.get("x86_64-unknown-linux-gnu"), Some(&"abc123"));
        assert_eq!(map.get("aarch64-unknown-linux-gnu"), Some(&"def456"));
        assert_eq!(map.get("nonsuch"), None);
    }

    #[test]
    fn empty_manifest_yields_no_entries() {
        assert!(parse_manifest("").is_empty());
    }
}
