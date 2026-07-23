//! `nula` CLI subcommands: `new`, `build`, `test`, `run`.
//!
//! All commands operate on the package rooted at the current directory
//! (except `new`, which creates one). Compiling and running is delegated to
//! the current `nulang` executable — the package manager only resolves
//! dependencies and picks the entry point.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::package::manifest::{Manifest, MANIFEST_FILE};
use crate::package::resolver::resolve;
use crate::types::{NuError, NuResult, Span};

/// Dispatch a `nula` invocation (`args` excludes the leading `nula`).
pub fn run(args: &[String]) -> NuResult<()> {
    match args.first().map(String::as_str) {
        Some("new") => cmd_new(args.get(1).map(String::as_str)),
        Some("build") => cmd_build(),
        Some("build-wasm") => cmd_build_wasm(),
        Some("test") => cmd_test(),
        Some("run") => cmd_run(),
        Some("--help") | Some("-h") => {
            print_usage();
            Ok(())
        }
        Some(other) => Err(NuError::PackageError { msg: format!(
            "unknown nula subcommand '{}' (expected new, build, build-wasm, test, or run)",
            other
        ), span: Span::default() }),
        None => {
            print_usage();
            Ok(())
        }
    }
}

fn print_usage() {
    println!("nula — the Nulang package manager");
    println!();
    println!("Usage: nulang nula <COMMAND>");
    println!();
    println!("Commands:");
    println!("  build        Resolve dependencies and type-check the package");
    println!("  build-wasm   Build package to .wasm + .cwasm (AOT, requires wasmtime)");
    println!("  test         Run every .nula file in the package's tests/ directory");
    println!("  run          Build and run the package entry point");
}

/// `nula new <name>`: scaffold a package directory.
fn cmd_new(name: Option<&str>) -> NuResult<()> {
    let name =
        name.ok_or_else(|| NuError::PackageError { msg: "nula new requires a package name".to_string(), span: Span::default() })?;
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(NuError::PackageError { msg: format!(
            "invalid package name '{}' (use letters, digits, '-' or '_')",
            name
        ), span: Span::default() });
    }
    let dir = PathBuf::from(name);
    if dir.exists() {
        return Err(NuError::PackageError { msg: format!(
            "directory '{}' already exists",
            name
        ), span: Span::default() });
    }
    scaffold_package(&dir, name)?;
    println!("Created package '{}'", name);
    Ok(())
}

/// Write the `Nulang.toml` + `src/main.nula` scaffold for a new package.
fn scaffold_package(dir: &Path, name: &str) -> NuResult<()> {
    let src_dir = dir.join("src");
    std::fs::create_dir_all(&src_dir).map_err(|e| {
        NuError::PackageError { msg: format!("cannot create {}: {}", src_dir.display(), e), span: Span::default() }
    })?;
    std::fs::write(
        dir.join(MANIFEST_FILE),
        format!(
            "[package]\nname = \"{}\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
            name
        ),
    )
    .map_err(|e| NuError::PackageError { msg: format!("cannot write {}: {}", MANIFEST_FILE, e), span: Span::default() })?;
    std::fs::write(
        src_dir.join("main.nula"),
        "// Run with: nulang nula run\n\nperform IO.print(\"Hello from Nulang!\")\n",
    )
    .map_err(|e| NuError::PackageError { msg: format!("cannot write main.nula: {}", e), span: Span::default() })?;
    Ok(())
}

/// Resolve the package in the current directory, write `Nulang.lock`, and
/// return the entry point path.
fn prepare_package() -> NuResult<PathBuf> {
    let root = std::env::current_dir()
        .map_err(|e| NuError::PackageError { msg: format!("cannot read current directory: {}", e), span: Span::default() })?;
    let manifest = Manifest::load(&root)?;
    let resolution = resolve(&root, &manifest)?;
    resolution.to_lockfile().save(&root)?;
    let entry = root.join(&manifest.package.entry);
    if !entry.exists() {
        return Err(NuError::PackageError { msg: format!(
            "entry point {} not found",
            entry.display()
        ), span: Span::default() });
    }
    Ok(entry)
}

/// Run the current `nulang` executable with `args`, inheriting stdio.
fn nulang_exe(args: &[&str]) -> NuResult<()> {
    let exe = std::env::current_exe()
        .map_err(|e| NuError::PackageError { msg: format!("cannot locate nulang executable: {}", e), span: Span::default() })?;
    let status = Command::new(exe)
        .args(args)
        .status()
        .map_err(|e| NuError::PackageError { msg: format!("failed to run nulang: {}", e), span: Span::default() })?;
    if !status.success() {
        return Err(NuError::PackageError { msg: format!(
            "nulang {} failed with status {}",
            args.join(" "),
            status
        ), span: Span::default() });
    }
    Ok(())
}

/// `nula build`: resolve dependencies, write the lockfile, type-check entry.
fn cmd_build() -> NuResult<()> {
    let entry = prepare_package()?;
    let entry_str = entry.to_string_lossy().into_owned();
    nulang_exe(&["--check", &entry_str])?;
    println!("Build finished.");
    Ok(())
}

/// `nula build-wasm`: compile package to .wasm + AOT .cwasm.
fn cmd_build_wasm() -> NuResult<()> {
    let entry = prepare_package()?;
    let entry_str = entry.to_string_lossy().into_owned();
    nulang_exe(&["--backend", "wasm-aot", &entry_str])?;
    println!("WASM AOT build finished.");
    Ok(())
}
/// `nula run`: build, then execute the entry point.
fn cmd_run() -> NuResult<()> {
    let entry = prepare_package()?;
    let entry_str = entry.to_string_lossy().into_owned();
    nulang_exe(&[&entry_str])
}

/// `nula test`: run every `.nula` file under the package's `tests/` directory.
fn cmd_test() -> NuResult<()> {
    let _entry = prepare_package()?;
    let tests_dir = std::env::current_dir()
        .map_err(|e| NuError::PackageError { msg: format!("cannot read current directory: {}", e), span: Span::default() })?
        .join("tests");
    let mut test_files: Vec<PathBuf> = match std::fs::read_dir(&tests_dir) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|ext| ext == "nula"))
            .collect(),
        Err(_) => Vec::new(),
    };
    test_files.sort();
    if test_files.is_empty() {
        println!(
            "No tests found ({} does not exist or has no .nula files).",
            tests_dir.display()
        );
        return Ok(());
    }
    let mut failed = 0;
    for file in &test_files {
        let file_str = file.to_string_lossy().into_owned();
        match nulang_exe(&[&file_str]) {
            Ok(()) => println!("ok   {}", file.display()),
            Err(e) => {
                failed += 1;
                println!("FAIL {} ({})", file.display(), e);
            }
        }
    }
    println!("{} passed, {} failed", test_files.len() - failed, failed);
    if failed > 0 {
        return Err(NuError::PackageError { msg: format!("{} test(s) failed", failed), span: Span::default() });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::manifest::DEFAULT_ENTRY;

    #[test]
    fn test_scaffold_package_creates_valid_manifest() {
        let dir = std::env::temp_dir().join(format!("nulang_nula_new_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        scaffold_package(&dir, "my-app").expect("scaffold should succeed");
        let manifest = Manifest::load(&dir).expect("scaffolded manifest should parse");
        assert_eq!(manifest.package.name, "my-app");
        assert_eq!(manifest.package.version, "0.1.0");
        assert_eq!(manifest.package.entry, DEFAULT_ENTRY);
        assert!(dir.join(DEFAULT_ENTRY).exists());

        let resolution = resolve(&dir, &manifest).expect("scaffold should resolve");
        assert_eq!(resolution.root().name, "my-app");
        assert!(resolution.to_lockfile().package.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_cmd_new_rejects_invalid_name() {
        let err = cmd_new(Some("../escape")).expect_err("path-like names are rejected");
        assert!(matches!(err, NuError::PackageError { msg: _, span: _ }));
        let err = cmd_new(None).expect_err("missing name is rejected");
        assert!(matches!(err, NuError::PackageError { msg: _, span: _ }));
    }

    #[test]
    fn test_print_usage_does_not_panic() {
        print_usage();
    }

    #[test]
    fn test_nulang_exe_rejects_invalid_args() {
        let result = nulang_exe(&["--nonexistent-flag"]);
        assert!(result.is_err(), "unknown flags should fail");
    }

    #[test]
    fn test_cmd_test_fails_in_non_package_dir() {
        let result = cmd_test();
        assert!(result.is_err(), "test outside package should fail");
    }
}
