//! Reading and writing the `Nulang.lock` lockfile.
//!
//! The lockfile pins the exact source each resolved dependency was fetched
//! from, so builds are reproducible:
//!
//! ```toml
//! version = 1
//!
//! [[package]]
//! name = "util"
//! version = "0.1.0"
//! source = "path+/home/david/projects/util"
//!
//! [[package]]
//! name = "json"
//! version = "0.2.0"
//! source = "git+https://github.com/example/json.nu.git#v0.2.0"
//! ```

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::types::{NuError, NuResult};

/// Lockfile name, written next to the root package's manifest.
pub const LOCKFILE_FILE: &str = "Nulang.lock";

/// Current on-disk lockfile format version.
pub const LOCKFILE_VERSION: u32 = 1;

/// A parsed `Nulang.lock`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lockfile {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub package: Vec<LockedPackage>,
}

/// One pinned dependency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    /// `path+<dir>` for local dependencies, `git+<url>#<rev>` for git ones.
    pub source: String,
    /// BLAKE3 hash of the resolved source (hex). A module pinned by content
    /// hash in 2026 is bit-identically resolvable in 2226 if any conforming
    /// registry mirrors that hash. Empty string if the hash was not computed
    /// (e.g. the source was unavailable at lock time).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content_hash: String,
}

impl Lockfile {
    /// An empty lockfile at the current format version.
    pub fn new() -> Self {
        Lockfile {
            version: LOCKFILE_VERSION,
            package: Vec::new(),
        }
    }

    /// Serialize to TOML text.
    pub fn to_toml(&self) -> NuResult<String> {
        toml::to_string_pretty(self)
            .map_err(|e| NuError::PackageError(format!("cannot serialize lockfile: {}", e)))
    }

    /// Parse lockfile TOML text.
    pub fn parse(source: &str) -> NuResult<Lockfile> {
        let lockfile: Lockfile = toml::from_str(source).map_err(|e| {
            NuError::PackageError(format!("invalid {}: {}", LOCKFILE_FILE, e))
        })?;
        if lockfile.version != LOCKFILE_VERSION {
            return Err(NuError::PackageError(format!(
                "unsupported {} version {} (expected {})",
                LOCKFILE_FILE, lockfile.version, LOCKFILE_VERSION
            )));
        }
        Ok(lockfile)
    }

    /// Write the lockfile into `dir`.
    pub fn save(&self, dir: &Path) -> NuResult<()> {
        let path = dir.join(LOCKFILE_FILE);
        std::fs::write(&path, self.to_toml()?).map_err(|e| {
            NuError::PackageError(format!("cannot write {}: {}", path.display(), e))
        })
    }

    /// Read the lockfile from `dir`.
    pub fn load(dir: &Path) -> NuResult<Lockfile> {
        let path = dir.join(LOCKFILE_FILE);
        let source = std::fs::read_to_string(&path).map_err(|e| {
            NuError::PackageError(format!("cannot read {}: {}", path.display(), e))
        })?;
        Self::parse(&source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_lockfile() -> Lockfile {
        Lockfile {
            version: LOCKFILE_VERSION,
            package: vec![
                LockedPackage {
                    name: "util".to_string(),
                    version: "0.1.0".to_string(),
                    source: "path+/home/david/projects/util".to_string(),
                    content_hash: "aabbcc".to_string(),
                },
                LockedPackage {
                    name: "json".to_string(),
                    version: "0.2.0".to_string(),
                    source: "git+https://github.com/example/json.nu.git#v0.2.0".to_string(),
                    content_hash: String::new(),
                },
            ],
        }
    }

    #[test]
    fn test_lockfile_toml_round_trip() {
        let lockfile = sample_lockfile();
        let toml_text = lockfile.to_toml().expect("lockfile should serialize");
        let parsed = Lockfile::parse(&toml_text).expect("lockfile should re-parse");
        assert_eq!(lockfile, parsed);
    }

    #[test]
    fn test_lockfile_file_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "nulang_lockfile_test_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir should be created");

        let lockfile = sample_lockfile();
        lockfile.save(&dir).expect("lockfile should save");
        assert!(dir.join(LOCKFILE_FILE).exists());

        let loaded = Lockfile::load(&dir).expect("lockfile should load");
        assert_eq!(lockfile, loaded);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_lockfile_rejects_unknown_version() {
        let source = "version = 99\n";
        let err = Lockfile::parse(source).expect_err("future versions must be rejected");
        match err {
            NuError::PackageError(msg) => assert!(msg.contains("version 99")),
            other => panic!("expected PackageError, got {:?}", other),
        }
    }

    #[test]
    fn test_lockfile_content_hash_round_trips() {
        // A non-empty content_hash must survive serialization + re-parse.
        let mut lockfile = Lockfile::new();
        lockfile.package.push(LockedPackage {
            name: "pinned".to_string(),
            version: "1.0.0".to_string(),
            source: "path+/tmp/pinned".to_string(),
            content_hash: "deadbeef".to_string(),
        });
        let toml_text = lockfile.to_toml().expect("serialize");
        assert!(toml_text.contains("content_hash"), "content_hash must be in TOML: {toml_text}");
        let parsed = Lockfile::parse(&toml_text).expect("parse");
        assert_eq!(parsed.package[0].content_hash, "deadbeef");
    }
}
