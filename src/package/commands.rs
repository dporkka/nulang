//! `nula` CLI subcommands: `new`, `init`, `build`, `build-wasm`, `test`, `run`,
//! `list`, `clean`, `add`, `remove`, `watch`.
//!
//! All commands operate on the package rooted at the current directory
//! (except `new` and `init`, which create one). Compiling and running is
//! delegated to the current `nulang` executable — the package manager only
//! resolves dependencies and picks the entry point.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::package::lockfile::{Lockfile, LOCKFILE_FILE};
use crate::package::manifest::{Dependency, DependencyDetail, Manifest, MANIFEST_FILE};
use crate::package::resolver::resolve;
use crate::types::{NuError, NuResult, Span};

/// Dispatch a `nula` invocation (`args` excludes the leading `nula`).
pub fn run(args: &[String]) -> NuResult<()> {
    match args.first().map(String::as_str) {
        Some("new") => cmd_new(args.get(1).map(String::as_str)),
        Some("init") => cmd_init(),
        Some("build") => cmd_build(),
        Some("build-wasm") => cmd_build_wasm(),
        Some("test") => {
            let filter = if args.get(1).map(String::as_str) == Some("--filter") {
                args.get(2).map(String::as_str)
            } else {
                None
            };
            cmd_test(filter)
        }
        Some("run") => {
            if args.get(1).map(String::as_str) == Some("--watch") {
                cmd_run_watch()
            } else {
                cmd_run()
            }
        }
        Some("watch") => cmd_run_watch(),
        Some("add") => {
            let name = args.get(1);
            let mut path: Option<String> = None;
            let mut git: Option<String> = None;
            let mut version: Option<String> = None;
            let mut i = 2;
            while i < args.len() {
                match args[i].as_str() {
                    "--path" => {
                        i += 1;
                        if i < args.len() {
                            path = Some(args[i].clone());
                        }
                    }
                    "--git" => {
                        i += 1;
                        if i < args.len() {
                            git = Some(args[i].clone());
                        }
                    }
                    "--version" => {
                        i += 1;
                        if i < args.len() {
                            version = Some(args[i].clone());
                        }
                    }
                    other => {
                        return Err(NuError::PackageError {
                            msg: format!("unknown flag '{}' for nula add", other),
                            span: Span::default(),
                        });
                    }
                }
                i += 1;
            }
            cmd_add(name, path.as_deref(), git.as_deref(), version.as_deref())
        }
        Some("remove") => cmd_remove(args.get(1).map(String::as_str)),
        Some("list") => cmd_list(),
        Some("clean") => cmd_clean(),
        Some("--help") | Some("-h") => {
            print_usage();
            Ok(())
        }
        Some(other) => Err(NuError::PackageError {
            msg: format!(
                "unknown nula subcommand '{}' (expected new, init, build, build-wasm, test, run, add, remove, watch, list, or clean)",
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
    println!("  test [--filter <substr>]  Run .nula test files (optionally filtered by name)");
    println!("  run           Build and run the package entry point");
    println!("  run --watch   Build and re-run on source changes");
    println!("  watch         Alias for 'run --watch'");
    println!("  add   <name>  Add a dependency to Nulang.toml");
    println!("  remove <name> Remove a dependency from Nulang.toml");
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
    let mut cmd = Command::new(&exe);
    cmd.args(args);
    // Auto-detect the stdlib directory relative to the executable so
    // that `import stdlib::*` works without setting NULANG_STDLIB.
    if std::env::var_os("NULANG_STDLIB").is_none() {
        if let Some(exe_dir) = exe.parent() {
            let candidate = exe_dir.join("stdlib");
            if candidate.is_dir() {
                cmd.env("NULANG_STDLIB", &candidate);
            }
        }
    }
    let status = cmd.status().map_err(|e| NuError::PackageError {
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

/// `nula run --watch` (or `nula watch`): build, run, and re-run when source
/// files change under `src/`. Uses simple mtime polling.
fn cmd_run_watch() -> NuResult<()> {
    let root = std::env::current_dir().map_err(|e| NuError::PackageError {
        msg: format!("cannot read current directory: {}", e),
        span: Span::default(),
    })?;
    let entry = prepare_package()?;
    let entry_str = entry.to_string_lossy().into_owned();

    // Initial run
    eprintln!("Building and running...");
    nulang_exe(&[&entry_str])?;

    // Collect initial mtimes for all .nula files under src/
    let src_dir = root.join("src");
    let mut last_mtimes = collect_mtimes(&src_dir);

    println!("watching... (Ctrl-C to stop)");
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let current = collect_mtimes(&src_dir);
        if current != last_mtimes {
            last_mtimes = current;
            eprintln!("\n--- change detected, rebuilding ---");
            // Re-resolve in case dependencies changed
            match prepare_package() {
                Ok(entry) => {
                    let es = entry.to_string_lossy().into_owned();
                    let _ = nulang_exe(&[&es]);
                }
                Err(e) => eprintln!("Error: {}", e),
            }
        }
    }
}

/// Collect (path, mtime) pairs for all .nula files under `dir`, sorted by path.
fn collect_mtimes(dir: &Path) -> Vec<(PathBuf, std::time::SystemTime)> {
    let mut result = Vec::new();
    collect_mtimes_recursive(dir, &mut result);
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

fn collect_mtimes_recursive(dir: &Path, out: &mut Vec<(PathBuf, std::time::SystemTime)>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_mtimes_recursive(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "nula") {
            if let Ok(meta) = std::fs::metadata(&path) {
                if let Ok(mtime) = meta.modified() {
                    out.push((path, mtime));
                }
            }
        }
    }
}

/// `nula test [--filter <substr>]`: discover and run `.nula` test files
/// under the package's `tests/` directory, reporting pass/fail.
///
/// Each test file is executed via the `nulang` exe in the current package
/// (same process as `nula run`). A test PASSes if it runs to completion
/// without error; any compile or runtime error (including assertion
/// failures from the `Test` effect) is a FAIL.
fn cmd_test(filter: Option<&str>) -> NuResult<()> {
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
            .filter(|p| {
                filter.map_or(true, |f| {
                    p.file_stem()
                        .and_then(|s| s.to_str())
                        .map_or(false, |s| s.contains(f))
                })
            })
            .collect(),
        Err(_) => Vec::new(),
    };
    test_files.sort();
    if test_files.is_empty() {
        println!("No tests found in {}", tests_dir.display());
        return Ok(());
    }
    eprintln!("running {} tests", test_files.len());
    let mut failed = 0;
    for file in &test_files {
        let file_str = file.to_string_lossy().into_owned();
        let relative = file
            .strip_prefix(&tests_dir.parent().unwrap_or(&tests_dir))
            .unwrap_or(file);
        match nulang_exe(&[&file_str]) {
            Ok(()) => println!("test {} ... ok", relative.display()),
            Err(e) => {
                failed += 1;
                let msg = e.to_string();
                // Extract the actual runtime error message from the NuError display
                // which includes span info; show the meaningful part.
                println!("test {} ... FAILED", relative.display());
                eprintln!("{}", msg);
            }
        }
    }
    println!(
        "\ntest result: {} passed; {} failed",
        test_files.len() - failed,
        failed
    );
    if failed > 0 {
        std::process::exit(1);
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

/// `nula add <name> [--path <p>] [--git <url>] [--version <v>]` — add or
/// update a dependency in `Nulang.toml`, then re-resolve and update
/// `Nulang.lock`.
fn cmd_add(
    name: Option<&String>,
    path: Option<&str>,
    git: Option<&str>,
    version: Option<&str>,
) -> NuResult<()> {
    let name = name.ok_or_else(|| NuError::PackageError {
        msg: "nula add requires a dependency name".to_string(),
        span: Span::default(),
    })?;
    validate_package_name(name)?;

    let root = std::env::current_dir().map_err(|e| NuError::PackageError {
        msg: format!("cannot read current directory: {}", e),
        span: Span::default(),
    })?;
    let manifest_path = root.join(MANIFEST_FILE);
    if !manifest_path.exists() {
        return Err(NuError::PackageError {
            msg: format!(
                "no {} found in {} — run 'nulang nula init' first",
                MANIFEST_FILE,
                root.display()
            ),
            span: Span::default(),
        });
    }
    let mut manifest = Manifest::load(&root).map_err(|e| NuError::PackageError {
        msg: format!("failed to load {}: {}", manifest_path.display(), e),
        span: Span::default(),
    })?;

    let dep = if path.is_some() || git.is_some() {
        Dependency::Detailed(DependencyDetail {
            path: path.map(|s| s.to_string()),
            git: git.map(|s| s.to_string()),
            version: version.map(|s| s.to_string()),
            ..Default::default()
        })
    } else {
        // Bare version dependency (or no flags -> version "*")
        Dependency::Version(version.unwrap_or("*").to_string())
    };

    let updated = manifest
        .dependencies
        .insert(name.to_string(), dep)
        .is_some();
    manifest.save(&root).map_err(|e| NuError::PackageError {
        msg: format!("failed to write {}: {}", manifest_path.display(), e),
        span: Span::default(),
    })?;

    if updated {
        println!("Updated dependency '{}' in {}.", name, MANIFEST_FILE);
    } else {
        println!("Added dependency '{}' to {}.", name, MANIFEST_FILE);
    }

    // Re-resolve and update the lockfile.
    eprintln!("  Resolving dependencies...");
    let resolution = resolve(&root, &manifest).map_err(|e| NuError::PackageError {
        msg: format!("failed to resolve dependencies: {}", e),
        span: Span::default(),
    })?;
    resolution
        .to_lockfile()
        .save(&root)
        .map_err(|e| NuError::PackageError {
            msg: format!(
                "failed to write {}: {}",
                root.join(LOCKFILE_FILE).display(),
                e
            ),
            span: Span::default(),
        })?;
    println!("  Lockfile updated.");
    Ok(())
}

/// `nula remove <name>` — remove a dependency from `Nulang.toml` and update
/// the lockfile.
fn cmd_remove(name: Option<&str>) -> NuResult<()> {
    let name = name.ok_or_else(|| NuError::PackageError {
        msg: "nula remove requires a dependency name".to_string(),
        span: Span::default(),
    })?;

    let root = std::env::current_dir().map_err(|e| NuError::PackageError {
        msg: format!("cannot read current directory: {}", e),
        span: Span::default(),
    })?;
    let manifest_path = root.join(MANIFEST_FILE);
    if !manifest_path.exists() {
        return Err(NuError::PackageError {
            msg: format!(
                "no {} found in {} — run 'nulang nula init' first",
                MANIFEST_FILE,
                root.display()
            ),
            span: Span::default(),
        });
    }
    let mut manifest = Manifest::load(&root).map_err(|e| NuError::PackageError {
        msg: format!("failed to load {}: {}", manifest_path.display(), e),
        span: Span::default(),
    })?;

    if manifest.dependencies.remove(name).is_none() {
        return Err(NuError::PackageError {
            msg: format!("dependency '{}' not found in {}", name, MANIFEST_FILE),
            span: Span::default(),
        });
    }

    manifest.save(&root).map_err(|e| NuError::PackageError {
        msg: format!("failed to write {}: {}", manifest_path.display(), e),
        span: Span::default(),
    })?;
    println!("Removed dependency '{}' from {}.", name, MANIFEST_FILE);

    // Re-resolve and update the lockfile.
    eprintln!("  Resolving dependencies...");
    let resolution = resolve(&root, &manifest).map_err(|e| NuError::PackageError {
        msg: format!("failed to resolve dependencies: {}", e),
        span: Span::default(),
    })?;
    resolution
        .to_lockfile()
        .save(&root)
        .map_err(|e| NuError::PackageError {
            msg: format!(
                "failed to write {}: {}",
                root.join(LOCKFILE_FILE).display(),
                e
            ),
            span: Span::default(),
        })?;
    println!("  Lockfile updated.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::manifest::DEFAULT_ENTRY;
    use std::sync::LazyLock;
    use std::sync::Mutex;

    /// Serialize tests that change the process CWD so they don't interfere
    /// with one another during parallel execution.
    static CWD_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));
    fn cwd_guard() -> std::sync::MutexGuard<'static, ()> {
        CWD_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }
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
        let _cwd = cwd_guard();
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
        // Use a temp dir with no Nulang.toml so prepare_package fails.
        let _cwd = cwd_guard();
        let dir = std::env::temp_dir().join(format!("nulang_no_pkg_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let _guard = ChangeDir::new(&dir);

        let result = cmd_test(None);
        assert!(result.is_err(), "test outside package should fail");

        let _ = std::fs::remove_dir_all(&dir);
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

    #[test]
    fn test_cmd_add_and_remove_dependency() {
        let _cwd = cwd_guard();
        let dir =
            std::env::temp_dir().join(format!("nulang_add_remove_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Create a stub dep package inside the test dir so the resolver can find it
        let dep_dir = dir.join("deps").join("mylib");
        scaffold_package(&dep_dir, "mylib").expect("scaffold dep should succeed");

        let _guard = ChangeDir::new(&dir);
        scaffold_package(&dir, "test-pkg").expect("scaffold should succeed");

        let manifest = Manifest::load(&dir).expect("manifest should load");
        assert!(manifest.dependencies.is_empty());

        // Add a path dep (relative: ./deps/mylib)
        let result = cmd_add(Some(&"mylib".to_string()), Some("./deps/mylib"), None, None);
        assert!(result.is_ok(), "add should succeed: {:?}", result.err());

        let manifest = Manifest::load(&dir).expect("manifest should load after add");
        assert!(manifest.dependencies.contains_key("mylib"));
        match &manifest.dependencies["mylib"] {
            Dependency::Detailed(d) => {
                assert_eq!(d.path.as_deref(), Some("./deps/mylib"));
            }
            _ => panic!("expected detailed dependency"),
        }

        // Remove the dep
        cmd_remove(Some("mylib")).expect("remove should succeed");
        let manifest = Manifest::load(&dir).expect("manifest should load after remove");
        assert!(!manifest.dependencies.contains_key("mylib"));

        // Remove a non-existent dep should fail
        let err = cmd_remove(Some("nonexistent")).expect_err("remove nonexistent should fail");
        assert!(matches!(err, NuError::PackageError { msg: _, span: _ }));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_manifest_add_dependency_direct() {
        // Test manifest-level mutation directly (avoiding resolver for git/version deps)
        let _cwd = cwd_guard();
        let dir =
            std::env::temp_dir().join(format!("nulang_manifest_dep_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let _guard = ChangeDir::new(&dir);

        scaffold_package(&dir, "test-pkg").expect("scaffold should succeed");

        // Add a detailed git dependency
        let mut manifest = Manifest::load(&dir).expect("manifest should load");
        manifest.dependencies.insert(
            "json".to_string(),
            Dependency::Detailed(DependencyDetail {
                git: Some("https://github.com/example/json.nu.git".to_string()),
                version: Some("0.2.0".to_string()),
                ..Default::default()
            }),
        );
        manifest.save(&dir).expect("save should succeed");

        // Reload and verify
        let manifest2 = Manifest::load(&dir).expect("manifest should reload");
        match &manifest2.dependencies["json"] {
            Dependency::Detailed(d) => {
                assert_eq!(
                    d.git.as_deref(),
                    Some("https://github.com/example/json.nu.git")
                );
                assert_eq!(d.version.as_deref(), Some("0.2.0"));
            }
            _ => panic!("expected detailed dependency"),
        }

        // Add a version-only dependency via fresh mutable load
        let mut manifest3 = Manifest::load(&dir).expect("manifest should load");
        manifest3.dependencies.insert(
            "registry-dep".to_string(),
            Dependency::Version("1.0.0".to_string()),
        );
        manifest3.save(&dir).expect("save should succeed");

        let manifest4 = Manifest::load(&dir).expect("manifest should reload");
        assert_eq!(
            manifest4.dependencies["registry-dep"],
            Dependency::Version("1.0.0".to_string())
        );

        // Remove and verify
        let mut manifest5 = Manifest::load(&dir).expect("manifest should load");
        assert!(manifest5.dependencies.remove("registry-dep").is_some());
        manifest5.save(&dir).expect("save should succeed");

        let manifest6 = Manifest::load(&dir).expect("manifest should reload");
        assert!(!manifest6.dependencies.contains_key("registry-dep"));
        assert!(manifest6.dependencies.contains_key("json"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_cmd_add_rejects_invalid_name() {
        let _cwd = cwd_guard();
        let dir =
            std::env::temp_dir().join(format!("nulang_add_invalid_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let _guard = ChangeDir::new(&dir);

        scaffold_package(&dir, "test-pkg").expect("scaffold should succeed");

        let err = cmd_add(Some(&"bad.name".to_string()), Some("./foo"), None, None)
            .expect_err("invalid name should fail");
        assert!(matches!(err, NuError::PackageError { msg: _, span: _ }));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_cmd_add_missing_name() {
        let _cwd = cwd_guard();
        let dir =
            std::env::temp_dir().join(format!("nulang_add_no_name_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let _guard = ChangeDir::new(&dir);

        scaffold_package(&dir, "test-pkg").expect("scaffold should succeed");

        let err = cmd_add(None, Some("./foo"), None, None).expect_err("missing name should fail");
        assert!(matches!(err, NuError::PackageError { msg: _, span: _ }));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
