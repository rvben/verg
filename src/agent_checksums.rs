//! Expected SHA-256 checksums for the verg-agent binaries of this version,
//! embedded at build time. Empty for local cargo (dev) builds, where checksum
//! verification is skipped.

use std::collections::HashMap;
use std::sync::OnceLock;

const EMBEDDED_MANIFEST: &str = include_str!(concat!(env!("OUT_DIR"), "/agent_checksums.txt"));

/// Parse a `sha256  verg-agent-<target>` manifest into target -> hash.
/// Both keys and values borrow from the `&'static str` manifest.
fn parse_manifest(manifest: &'static str) -> HashMap<&'static str, &'static str> {
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

/// Parsed manifest, computed once and reused for all lookups.
static MANIFEST: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();

/// Expected hash for an arch target triple, or None if not embedded.
pub fn expected_sha256(arch_target: &str) -> Option<&'static str> {
    MANIFEST
        .get_or_init(|| parse_manifest(EMBEDDED_MANIFEST))
        .get(arch_target)
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_manifest_lines() {
        let map = parse_manifest(
            "abc123  verg-agent-x86_64-unknown-linux-gnu\n\
             def456  verg-agent-aarch64-unknown-linux-gnu\n",
        );
        assert_eq!(map.get("x86_64-unknown-linux-gnu"), Some(&"abc123"));
        assert_eq!(map.get("aarch64-unknown-linux-gnu"), Some(&"def456"));
        assert_eq!(map.get("nonsuch"), None);
    }

    #[test]
    fn empty_manifest_yields_no_entries() {
        assert!(parse_manifest("").is_empty());
    }

    /// `expected_sha256` must return the same result on every call (the OnceLock
    /// is initialized exactly once and all subsequent calls reuse it).
    #[test]
    fn expected_sha256_is_idempotent() {
        // Unknown arch always returns None, regardless of call count.
        let first = expected_sha256("not-a-real-arch-triple");
        let second = expected_sha256("not-a-real-arch-triple");
        assert_eq!(first, None);
        assert_eq!(second, None);

        // Any two calls for the same arch return identical values (pointer equality
        // holds because both point into the static manifest string).
        let a = expected_sha256("x86_64-unknown-linux-gnu");
        let b = expected_sha256("x86_64-unknown-linux-gnu");
        assert_eq!(a, b);
    }

    /// When the manifest is non-empty, `parse_manifest` returns the correct hash
    /// for a known arch and None for an unknown one.
    #[test]
    fn parse_manifest_known_and_unknown_arch() {
        let map = parse_manifest(
            "deadbeef  verg-agent-x86_64-unknown-linux-musl\n\
             cafebabe  verg-agent-aarch64-unknown-linux-musl\n",
        );
        assert_eq!(
            map.get("x86_64-unknown-linux-musl"),
            Some(&"deadbeef"),
            "known arch must return its hash"
        );
        assert_eq!(
            map.get("riscv64gc-unknown-linux-gnu"),
            None,
            "unknown arch must return None"
        );
    }
}
