//! Parsing of `Nulang.toml` package manifests.
//!
//! A manifest looks like:
//!
//! ```toml
//! [package]
//! name = "my-app"
//! version = "0.1.0"
//! entry = "src/main.nula"   # optional; this is the default
//!
//! [dependencies]
//! util = { path = "../util" }
//! json = { git = "https://github.com/example/json.nu.git", tag = "v0.2.0" }
//! ```

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use crate::types::{NuError, NuResult};

/// Manifest file name, expected at the root of every package.
pub const MANIFEST_FILE: &str = "Nulang.toml";

/// Default entry point, relative to the package root.
pub const DEFAULT_ENTRY: &str = "src/main.nula";

/// A parsed `Nulang.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    pub package: PackageSection,
    #[serde(default)]
    pub dependencies: BTreeMap<String, Dependency>,
}

/// The `[package]` section.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PackageSection {
    pub name: String,
    pub version: String,
    /// Entry point relative to the package root; `src/main.nula` when omitted.
    #[serde(default = "default_entry")]
    pub entry: String,
}

fn default_entry() -> String {
    DEFAULT_ENTRY.to_string()
}

/// One entry in `[dependencies]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Dependency {
    /// `foo = "0.1.0"` — a bare version requirement. The MVP has no network
    /// registry, so these parse but fail at resolution time.
    Version(String),
    /// `foo = { path = "../foo" }` or `foo = { git = "...", ... }`.
    Detailed(DependencyDetail),
}

/// Table form of a dependency: a local path, a git URL, or both refined by a
/// version requirement.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct DependencyDetail {
    pub path: Option<String>,
    pub git: Option<String>,
    pub rev: Option<String>,
    pub branch: Option<String>,
    pub tag: Option<String>,
    pub version: Option<String>,
}

impl Manifest {
    /// Parse a manifest from its TOML text.
    pub fn parse(source: &str) -> NuResult<Manifest> {
        toml::from_str(source)
            .map_err(|e| NuError::PackageError(format!("invalid {}: {}", MANIFEST_FILE, e)))
    }

    /// Load and parse the manifest in `dir`.
    pub fn load(dir: &Path) -> NuResult<Manifest> {
        let path = dir.join(MANIFEST_FILE);
        let source = std::fs::read_to_string(&path).map_err(|e| {
            NuError::PackageError(format!("cannot read {}: {}", path.display(), e))
        })?;
        Self::parse(&source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_parse_minimal() {
        let source = r#"
            [package]
            name = "my-app"
            version = "0.1.0"
        "#;
        let manifest = Manifest::parse(source).expect("minimal manifest should parse");
        assert_eq!(manifest.package.name, "my-app");
        assert_eq!(manifest.package.version, "0.1.0");
        assert_eq!(manifest.package.entry, DEFAULT_ENTRY);
        assert!(manifest.dependencies.is_empty());
    }

    #[test]
    fn test_manifest_parse_with_dependencies() {
        let source = r#"
            [package]
            name = "my-app"
            version = "0.2.0"
            entry = "src/app.nula"

            [dependencies]
            util = { path = "../util" }
            json = { git = "https://github.com/example/json.nu.git", tag = "v0.2.0" }
            fancy = { git = "https://example.com/fancy.git", rev = "abc123", version = "1.0.0" }
            registry_dep = "0.3.0"
        "#;
        let manifest = Manifest::parse(source).expect("manifest with deps should parse");
        assert_eq!(manifest.package.entry, "src/app.nula");
        assert_eq!(manifest.dependencies.len(), 4);

        let util = &manifest.dependencies["util"];
        assert_eq!(
            *util,
            Dependency::Detailed(DependencyDetail {
                path: Some("../util".to_string()),
                ..Default::default()
            })
        );

        let json = &manifest.dependencies["json"];
        match json {
            Dependency::Detailed(d) => {
                assert_eq!(d.git.as_deref(), Some("https://github.com/example/json.nu.git"));
                assert_eq!(d.tag.as_deref(), Some("v0.2.0"));
                assert_eq!(d.path, None);
            }
            Dependency::Version(_) => panic!("json should be a detailed dependency"),
        }

        assert_eq!(
            manifest.dependencies["registry_dep"],
            Dependency::Version("0.3.0".to_string())
        );
    }

    #[test]
    fn test_manifest_parse_missing_name_fails() {
        let source = r#"
            [package]
            version = "0.1.0"
        "#;
        let err = Manifest::parse(source).expect_err("name is required");
        match err {
            NuError::PackageError(msg) => assert!(msg.contains(MANIFEST_FILE)),
            other => panic!("expected PackageError, got {:?}", other),
        }
    }

    #[test]
    fn test_manifest_parse_invalid_toml_fails() {
        let err = Manifest::parse("not [valid toml").expect_err("garbage should not parse");
        assert!(matches!(err, NuError::PackageError(_)));
    }
}
