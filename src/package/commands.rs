//! `nula` CLI subcommands: `new`, `init`, `build`, `build-wasm`, `test`, `run`,
//! `list`, `clean`.
//!
//! All commands operate on the package rooted at the current directory
//! (except `new` and `init`, which create one). Compiling and running is
//! delegated to the current `nulang` executable — the package manager only
//! resolves dependencies and picks the entry point.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::package::lockfile::{Lockfile, LOCKFILE_FILE};
use crate::package::manifest::{Manifest, MANIFEST_FILE};
use crate::package::resolver::resolve;
use crate::types::{NuError, NuResult, Span};

/// Dispatch a `nula` invocation (`args` excludes the leading `nula`).
pub fn run(args: &[String]) -> NuResult<()> {
    match args.first().map(String::as_str) {
        Some("new") => cmd_new(args.get(1).map(String::as_str)),
        Some("init") => cmd_init(),
        Some("build") => cmd_build(),
        Some("build-wasm") => cmd_build_wasm(),
        Some("test") => cmd_test(),
        Some("run") => cmd_run(),
        Some("list") => cmd_list(),
        Some("clean") => cmd_clean(),
        Some("--help") | Some("-h") => {
            print_usage();
            Ok(())
        }
        Some(other) => Err(NuError::PackageError {
            msg: format!(
                "unknown nula subcommand '{}' (expected new, init, build, build-wasm, test, run, list, or clean)",
                other
            ),
            span: Span::default(),
        }),
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
    println!("  new   <path>  Scaffold a new package directory");
    println!("  init          Scaffold a new package in the current directory");
    println!("  build         Resolve dependencies and type-check the package");
    println!("  build-wasm    Build package to .wasm + .cwasm (AOT, requires wasmtime)");
    println!("  test          Run every .nula file in the package's tests/ directory");
    println!("  run           Build and run the package entry point");
    println!("  list          List resolved dependencies from Nulang.lock");
    println!("  clean         Remove build artifacts (.nbc files)");
}

/// `nula new <name>`: scaffold a package directory.
fn cmd_new(path_arg: Option<&str>) -> NuResult<()> {
    let path_str = path_arg.ok_or_else(|| NuError::PackageError {
        msg: "nula new requires a package name or path".to_string(),
        span: Span::default(),
    })?;
    let dir = PathBuf::from(path_str);
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| NuError::PackageError {
            msg: format!("invalid path '{}' — cannot extract package name", path_str),
            span: Span::default(),
        })?;
    validate_package_name(name)?;
    if dir.exists() {
        return Err(NuError::PackageError {
            msg: format!("directory '{}' already exists", dir.display()),
            span: Span::default(),
        });
    }
    scaffold_package(&dir, name)?;
    println!("Created package '{}' at '{}'", name, dir.display());
    Ok(())
}

/// `nula init`: scaffold a package in the current directory.
fn cmd_init() -> NuResult<()> {
    let dir = std::env::current_dir().map_err(|e| NuError::PackageError {
        msg: format!("cannot read current directory: {}", e),
        span: Span::default(),
    })?;
    let manifest_path = dir.join(MANIFEST_FILE);
    if manifest_path.exists() {
        return Err(NuError::PackageError {
            msg: format!(
                "{} already exists in {} — package is already initialized",
                MANIFEST_FILE,
                dir.display()
            ),
            span: Span::default(),
        });
    }
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("nulang-project");
    validate_package_name(name)?;
    scaffold_package(&dir, name)?;
    // Write a basic .gitignore
    let gitignore = dir.join(".gitignore");
    if !gitignore.exists() {
        let _ = std::fs::write(&gitignore, "# Nulang build artifacts\n*.nbc\n.nula/\n");
    }
    println!("Initialized package '{}' in '{}'", name, dir.display());
    Ok(())
}

fn validate_package_name(name: &str) -> NuResult<()> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(NuError::PackageError {
            msg: format!(
                "invalid package name '{}' (use letters, digits, '-' or '_')",
                name
            ),
            span: Span::default(),
        });
    }
    Ok(())
}

/// Write the `Nulang.toml` + `src/main.nula` scaffold for a new package.
fn scaffold_package(dir: &Path, name: &str) -> NuResult<()> {
    let src_dir = dir.join("src");
    std::fs::create_dir_all(&src_dir).map_err(|e| NuError::PackageError {
        msg: format!("cannot create {}: {}", src_dir.display(), e),
        span: Span::default(),
    })?;
    let manifest_path = dir.join(MANIFEST_FILE);
    std::fs::write(
        &manifest_path,
        format!(
            "[package]\nname = \"{}\"\nversion = \"0.1.0\"\n\n[dependencies]\n",
            name
        ),
    )
    .map_err(|e| NuError::PackageError {
        msg: format!("cannot write {}: {}", manifest_path.display(), e),
        span: Span::default(),
    })?;
    let main_path = src_dir.join("main.nula");
    std::fs::write(
        &main_path,
        "// Run with: nulang nula run\n\nperform IO.print(\"Hello from Nulang!\")\n",
    )
    .map_err(|e| NuError::PackageError {
        msg: format!("cannot write {}: {}", main_path.display(), e),
        span: Span::default(),
    })?;
    Ok(())
}

/// Resolve the package in the current directory, write `Nulang.lock`, and
/// return the entry point path.
fn prepare_package() -> NuResult<PathBuf> {
    let root = std::env::current_dir().map_err(|e| NuError::PackageError {
        msg: format!("cannot read current directory: {}", e),
        span: Span::default(),
    })?;
    let manifest_path = root.join(MANIFEST_FILE);
    let manifest = Manifest::load(&root).map_err(|e| NuError::PackageError {
        msg: format!(
            "failed to load {} at {}: {}",
            MANIFEST_FILE,
            root.display(),
            e
        ),
        span: Span::default(),
    })?;

    eprintln!("  Resolving dependencies...");
    let resolution = resolve(&root, &manifest).map_err(|e| NuError::PackageError {
        msg: format!(
            "failed to resolve dependencies for package '{}': {}\n  help: check that all [dependencies] in {} are reachable",
            manifest.package.name,
            e,
            manifest_path.display()
        ),
        span: Span::default(),
    })?;

    let lock_path = root.join(LOCKFILE_FILE);
    resolution
        .to_lockfile()
        .save(&root)
        .map_err(|e| NuError::PackageError {
            msg: format!("failed to write {}: {}", lock_path.display(), e),
            span: Span::default(),
        })?;

    let entry = root.join(&manifest.package.entry);
    if !entry.exists() {
        return Err(NuError::PackageError {
            msg: format!(
                "entry point '{}' not found (defined as `entry = \"{}\"` in {})",
                entry.display(),
                manifest.package.entry,
                manifest_path.display()
            ),
            span: Span::default(),
        });
    }
    Ok(entry)
}

/// Run the current `nulang` executable with `args`, inheriting stdio.
fn nulang_exe(args: &[&str]) -> NuResult<()> {
    let exe = std::env::current_exe().map_err(|e| NuError::PackageError {
        msg: format!("cannot locate nulang executable: {}", e),
        span: Span::default(),
    })?;
    let status = Command::new(&exe)
        .args(args)
        .status()
        .map_err(|e| NuError::PackageError {
            msg: format!("failed to run nulang ({}): {}", exe.display(), e),
            span: Span::default(),
        })?;
    if !status.success() {
        return Err(NuError::PackageError {
            msg: format!("nulang {} exited with {}", args.join(" "), status),
            span: Span::default(),
        });
    }
    Ok(())
}

/// `nula build`: resolve dependencies, write the lockfile, type-check entry.
fn cmd_build() -> NuResult<()> {
    eprintln!("Building...");
    let entry = prepare_package()?;
    let entry_str = entry.to_string_lossy().into_owned();
    eprintln!("  Type-checking {}...", entry.display());
    nulang_exe(&["--check", &entry_str])?;
    println!("Build succeeded.");
    Ok(())
}

/// `nula build-wasm`: compile package to .wasm + AOT .cwasm.
fn cmd_build_wasm() -> NuResult<()> {
    eprintln!("Building (WASM AOT)...");
    let entry = prepare_package()?;
    let entry_str = entry.to_string_lossy().into_owned();
    eprintln!("  Compiling {} to WASM...", entry.display());
    nulang_exe(&["--backend", "wasm-aot", &entry_str])?;
    println!("WASM AOT build succeeded.");
    Ok(())
}

/// `nula run`: build, then execute the entry point.
fn cmd_run() -> NuResult<()> {
    eprintln!("Building and running...");
    let entry = prepare_package()?;
    let entry_str = entry.to_string_lossy().into_owned();
    nulang_exe(&[&entry_str])
}

/// `nula test`: run every `.nula` file under the package's `tests/` directory.
fn cmd_test() -> NuResult<()> {
    eprintln!("Preparing package...");
    let _entry = prepare_package()?;
    let tests_dir = std::env::current_dir()
        .map_err(|e| NuError::PackageError {
            msg: format!("cannot read current directory: {}", e),
            span: Span::default(),
        })?
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
    eprintln!("Running {} test(s)...", test_files.len());
    let mut failed = 0;
    for file in &test_files {
        let file_str = file.to_string_lossy().into_owned();
        let relative = file
            .strip_prefix(&tests_dir.parent().unwrap_or(&tests_dir))
            .unwrap_or(file);
        match nulang_exe(&[&file_str]) {
            Ok(()) => println!("  ok   {}", relative.display()),
            Err(e) => {
                failed += 1;
                println!("  FAIL {} ({})", relative.display(), e);
            }
        }
    }
    println!("{} passed, {} failed", test_files.len() - failed, failed);
    if failed > 0 {
        return Err(NuError::PackageError {
            msg: format!("{} test(s) failed", failed),
            span: Span::default(),
        });
    }
    Ok(())
}

/// `nula list`: print all locked dependencies with versions and sources.
fn cmd_list() -> NuResult<()> {
    let root = std::env::current_dir().map_err(|e| NuError::PackageError {
        msg: format!("cannot read current directory: {}", e),
        span: Span::default(),
    })?;
    let lock_path = root.join(LOCKFILE_FILE);
    let lockfile = Lockfile::load(&root).map_err(|e| NuError::PackageError {
        msg: format!(
            "failed to read {}: {}\n  hint: run 'nulang nula build' first to generate it",
            lock_path.display(),
            e
        ),
        span: Span::default(),
    })?;
    if lockfile.package.is_empty() {
        println!("No dependencies locked.");
        return Ok(());
    }
    println!("Locked dependencies (from {}):", lock_path.display());
    for pkg in &lockfile.package {
        println!("  {} v{} — {}", pkg.name, pkg.version, pkg.source);
    }
    Ok(())
}

/// `nula clean`: remove build artifacts (.nbc files).
fn cmd_clean() -> NuResult<()> {
    let root = std::env::current_dir().map_err(|e| NuError::PackageError {
        msg: format!("cannot read current directory: {}", e),
        span: Span::default(),
    })?;
    eprintln!("Cleaning build artifacts...");
    let mut removed = 0u64;
    remove_nbc_files(&root, &mut removed);
    if removed == 0 {
        println!("No build artifacts found.");
    } else {
        println!("Removed {} build artifact(s).", removed);
    }
    Ok(())
}

/// Recursively remove .nbc files under `dir`.
fn remove_nbc_files(dir: &Path, count: &mut u64) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip .git and .nula directories
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name == ".git" || name == ".nula" {
                    continue;
                }
            }
            remove_nbc_files(&path, count);
        } else if path.extension().is_some_and(|ext| ext == "nbc") {
            if std::fs::remove_file(&path).is_ok() {
                eprintln!("  Removed {}", path.display());
                *count += 1;
            }
        }
    }
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
        // Path with invalid package name (contains '.')
        let err = cmd_new(Some("./my.app")).expect_err("dots in name are rejected");
        assert!(matches!(err, NuError::PackageError { msg: _, span: _ }));
        let err = cmd_new(None).expect_err("missing name is rejected");
        assert!(matches!(err, NuError::PackageError { msg: _, span: _ }));
    }

    #[test]
    fn test_cmd_new_accepts_path() {
        let dir = std::env::temp_dir().join(format!("nulang_new_path_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path_str = dir.to_str().expect("temp dir should be valid UTF-8");
        let result = cmd_new(Some(path_str));
        assert!(
            result.is_ok(),
            "path with valid basename should succeed: {:?}",
            result.err()
        );
        assert!(dir.join("Nulang.toml").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_cmd_init_creates_in_current_dir() {
        let dir = std::env::temp_dir().join(format!("nulang_init_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let _guard = ChangeDir::new(&dir);

        cmd_init().expect("init in empty dir should succeed");
        assert!(dir.join("Nulang.toml").exists());
        assert!(dir.join("src/main.nula").exists());
        assert!(dir.join(".gitignore").exists());

        // Second init should fail
        let err = cmd_init().expect_err("second init should fail");
        assert!(matches!(err, NuError::PackageError { msg: _, span: _ }));

        let _ = std::fs::remove_dir_all(&dir);
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

    /// Helper: temporarily change the current directory, restoring it on drop.
    struct ChangeDir {
        original: PathBuf,
    }

    impl ChangeDir {
        fn new(dir: &Path) -> Self {
            let original = std::env::current_dir().unwrap();
            std::env::set_current_dir(dir).unwrap();
            ChangeDir { original }
        }
    }

    impl Drop for ChangeDir {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.original);
        }
    }
}
