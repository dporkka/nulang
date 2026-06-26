# Nulang Package Manager Design Document

## Overview

The Nulang Package Manager (`nu`) is the official package management and build tool for the Nulang programming language. Modeled after Rust's Cargo, Node's npm, and Elixir's Mix, `nu` provides a unified interface for project creation, dependency management, compilation, testing, and publishing. It is deeply integrated with Nulang's module system, compiler, and standard library, providing a seamless developer experience from project scaffolding to production deployment.

**Version:** 1.0.0  
**Status:** Design Complete — Ready for Implementation  
**Target Nulang Edition:** 2024

---

## Table of Contents

1. [Core Concepts](#1-core-concepts)
2. [Architecture Overview](#2-architecture-overview)
3. [CLI Design & Commands](#3-cli-design--commands)
4. [API Design & Specification](#4-api-design--specification)
5. [Module Reference](#5-module-reference)
6. [Implementation Phases](#6-implementation-phases)
7. [Appendices](#7-appendices)

---

## 1. Core Concepts

### 1.1 Package

A **Package** is the fundamental unit of code distribution in the Nulang ecosystem. A package contains Nulang source code, a manifest file (`Nulang.toml`), metadata, and optionally assets, tests, and documentation. Packages are versioned using Semantic Versioning (SemVer) and published to the Nulang Registry.

```
my-package/
├── Nulang.toml          # Package manifest
├── Nulang.lock          # Dependency lock file
├── src/
│   ├── main.nl          # Library entry point (for executables)
│   ├── lib.nl           # Library root (for libraries)
│   └── internal/
│       └── helpers.nl   # Private modules
├── tests/
│   ├── unit_tests.nl    # Unit tests
│   └── integration.nl   # Integration tests
├── benches/
│   └── benchmark.nl     # Performance benchmarks
├── examples/
│   ├── basic.nl         # Usage examples
│   └── advanced.nl
├── docs/
│   ├── guide.md         # Documentation
│   └── api/
├── scripts/
│   └── build.sh         # Custom build scripts
├── build/               # Build output (generated)
│   └── debug/
└── README.md
```

### 1.2 Manifest (Nulang.toml)

The **Manifest** is a TOML configuration file that declares a package's metadata, dependencies, build configuration, and scripts. It is the source of truth for how a package is built, tested, and published.

```toml
[package]
name = "http-client"
version = "1.2.3"
edition = "2024"
authors = ["Alice <alice@example.com>", "Bob <bob@example.com>"]
license = "MIT"
description = "A high-performance HTTP client for Nulang"
repository = "https://github.com/nulang/http-client"
documentation = "https://docs.nulang.org/http-client"
readme = "README.md"
keywords = ["http", "client", "networking"]
categories = ["network-programming", "web-apis"]
exclude = ["tests/fixtures/*", "scripts/*"]
include = ["src/**", "docs/**", "README.md", "LICENSE"]

[dependencies]
async-runtime = "^2.1.0"
tls = { version = "^1.0", features = ["rustls"] }
serde = { version = "^3.0", optional = true }

[dev-dependencies]
test-server = "^1.0"
mock-all = "^2.0"

[build-dependencies]
protoc = "^3.0"

[features]
default = ["json"]
json = ["serde"]
compression = ["async-runtime/compression"]
full = ["json", "compression"]

[scripts]
build = "nu build --release"
test = "nu test --all-features"
lint = "nu check && nu clippy"
doc = "nu doc --open"
fmt = "nu fmt"
pre-commit = "nu fmt && nu check && nu test"

[profile.release]
opt_level = 3
lto = true
debug = false
strip = true
panic = "abort"

[profile.dev]
opt_level = 0
debug = true
incremental = true

[profile.test]
debug = true
coverage = true

[workspace]
members = ["packages/*", "tools/*"]
resolver = "2"

[registry]
index = "https://registry.nulang.org/api/v1"
publish = true
```

### 1.3 Dependency Resolution

**Dependency Resolution** is the process of computing a compatible set of package versions from the dependency constraints declared in `Nulang.toml`. The resolver uses a SAT-solver-inspired algorithm to find the optimal version set, prioritizing:

1. **Correctness**: All constraints must be satisfied
2. **Recency**: Prefer newer versions within constraints
3. **Minimality**: Minimize total dependency count
4. **Conflict reporting**: Clear error messages for conflicts

```
+--------------------------------------------------------------------+
|                    Dependency Resolution                            |
+--------------------------------------------------------------------+
|                                                                     |
|  Input: Nulang.toml dependencies                                    |
|                                                                     |
|  http = "^1.2.0"      -> [1.2.0, 1.3.0, 1.4.0]                    |
|  json = "^2.0.0"      -> [2.0.0, 2.1.0]                           |
|  database = ">=0.5"   -> [0.5.0, 0.6.0, 1.0.0]                    |
|                                                                     |
|  Resolution process:                                                |
|                                                                     |
|  1. Select http@1.4.0 (newest in range)                             |
|     -> http@1.4.0 requires json >= 1.5                             |
|                                                                     |
|  2. Select json@2.1.0 (newest, satisfies both)                      |
|                                                                     |
|  3. Select database@1.0.0 (newest in range)                         |
|     -> database@1.0.0 requires http ^1.0 (compatible)              |
|                                                                     |
|  Output: Nulang.lock with exact versions                            |
|                                                                     |
|  [[package]]                                                        |
|  name = "http"                                                      |
|  version = "1.4.0"                                                  |
|  source = "registry+https://registry.nulang.org"                    |
|                                                                     |
|  [[package]]                                                        |
|  name = "json"                                                      |
|  version = "2.1.0"                                                  |
|  source = "registry+https://registry.nulang.org"                    |
|                                                                     |
|  [[package]]                                                        |
|  name = "database"                                                  |
|  version = "1.0.0"                                                  |
|  source = "registry+https://registry.nulang.org"                    |
|                                                                     |
+--------------------------------------------------------------------+
```

### 1.4 Workspace

A **Workspace** is a collection of related packages that share a common `Nulang.toml` and lock file. Workspaces enable monorepo development where multiple packages can depend on each other and be built/tested together.

```
my-monorepo/
├── Nulang.toml          # Workspace root
├── Nulang.lock          # Shared lock file
├── packages/
│   ├── core/
│   │   ├── Nulang.toml
│   │   └── src/
│   ├── http/
│   │   ├── Nulang.toml  # depends on core
│   │   └── src/
│   └── database/
│       ├── Nulang.toml  # depends on core
│       └── src/
├── tools/
│   ├── cli/
│   │   ├── Nulang.toml  # depends on core, http
│   │   └── src/
│   └── migration/
│       ├── Nulang.toml
│       └── src/
└── apps/
    ├── web/
    │   ├── Nulang.toml  # depends on core, http, database
    │   └── src/
    └── worker/
        ├── Nulang.toml
        └── src/
```

### 1.5 Lock File

The **Lock File** (`Nulang.lock`) captures the exact versions of all dependencies, including transitive ones. It ensures that every build uses the same dependency versions, providing deterministic and reproducible builds across all environments.

### 1.6 Registry

The **Registry** is the central package index at `registry.nulang.org`. It stores package metadata, version information, and downloadable archives. Packages are published to the registry after authentication and validation.

---

## 2. Architecture Overview

### 2.1 System Architecture

```
+============================================================================+
|                    Nulang Package Manager Architecture                     |
+============================================================================+
|                                                                            |
|  +------------------+  +------------------+  +-------------------------+  |
|  |   CLI Interface  |  |   Commands       |  |   Output                |  |
|  |                  |  |                  |  |                         |  |
|  |  nu new          |  |  - new           |  |  - Console output        |  |
|  |  nu build        |  |  - build         |  |  - Progress bars         |  |
|  |  nu test         |  |  - test          |  |  - Error formatting      |  |
|  |  nu add          |  |  - add/remove    |  |  - JSON output (--json)  |  |
|  |  nu publish      |  |  - publish       |  |  - Colored output        |  |
|  |  ...             |  |  - run           |  |                         |  |
|  +--------+---------+  +--------+---------+  +-------------------------+  |
|           |                     |                                         |
+-----------+---------------------+-----------------------------------------+
|                                                                            |
|  +------------------+  +------------------+  +-------------------------+  |
|  |   Core Engine    |  |   Dependency     |  |   Build System          |  |
|  |                  |  |   Resolution     |  |                         |  |
|  |  - Manifest      |  |                  |  |  - Source compilation   |  |
|  |    parser        |  |  - SAT solver    |  |  - Incremental builds   |  |
|  |  - Project       |  |  - Version       |  |  - Parallel jobs        |  |
|  |    discovery     |  |    parsing       |  |  - Artifact caching     |  |
|  |  - Lock file     |  |  - Conflict      |  |  - Profile management   |  |
|  |    management    |  |    detection     |  |  - Linking              |  |
|  +--------+---------+  +--------+---------+  +-------------------------+  |
|           |                     |                                         |
+-----------+---------------------+-----------------------------------------+
|                                                                            |
|  +------------------+  +------------------+  +-------------------------+  |
|  |   Registry       |  |   Workspace      |  |   Quality Assurance     |  |
|  |   Client         |  |   Manager        |  |                         |  |
|  |                  |  |                  |  |  - Test runner           |  |
|  |  - HTTP client   |  |  - Member        |  |  - Benchmark runner      |  |
|  |  - Authentication|  |    discovery     |  |  - Linter                |  |
|  |  - Package       |  |  - Inter-package |  |  - Formatter             |  |
|  |    download      |  |    dependency    |  |  - Documentation         |  |
|  |  - Package       |  |    resolution    |  |    generator             |  |
|  |    upload        |  |  - Shared lock   |  |  - Type checker          |  |
|  |  - Search API    |  |    file          |  |  - Security audit        |  |
|  +------------------+  +------------------+  +-------------------------+  |
|                                                                            |
+============================================================================+
```

### 2.2 Build Pipeline

```
+---------------------------------------------------------------------+
|                       Build Pipeline                                 |
+---------------------------------------------------------------------+
|                                                                      |
|  Source Files                                                        |
|     |                                                                |
|     v                                                                |
|  +------------------+                                                |
|  | 1. Parse         |  Parse .nl files into AST                      |
|  |    Manifest      |  Read Nulang.toml for config                    |
|  +--------+---------+                                                |
|           |                                                          |
|           v                                                          |
|  +------------------+                                                |
|  | 2. Resolve Deps  |  Download dependencies                         |
|  |                  |  Check lock file                               |
|  +--------+---------+                                                |
|           |                                                          |
|           v                                                          |
|  +------------------+                                                |
|  | 3. Type Check    |  Type inference and validation                 |
|  |                  |  Cross-module type checking                    |
|  +--------+---------+                                                |
|           |                                                          |
|           v                                                          |
|  +------------------+                                                |
|  | 4. Compile       |  AST -> Intermediate IR                        |
|  |                  |  IR -> Native code / WASM                      |
|  +--------+---------+                                                |
|           |                                                          |
|           v                                                          |
|  +------------------+                                                |
|  | 5. Link          |  Link with dependencies                        |
|  |                  |  Create executable or library                  |
|  +--------+---------+                                                |
|           |                                                          |
|           v                                                          |
|  +------------------+                                                |
|  | 6. Cache         |  Store compiled artifacts                      |
|  |                  |  Generate build summary                        |
|  +------------------+                                                |
|                                                                      |
+---------------------------------------------------------------------+
```

### 2.3 Data Flow Diagram

```
+----------+     +----------+     +----------+     +----------+
| Nulang   | --> | Manifest | --> | Resolver | --> | Registry |
| .toml    |     | Parser   |     | Engine   |     | Client   |
+----------+     +----------+     +----------+     +----------+
                                      |                  |
                                      v                  v
+----------+     +----------+     +----------+     +----------+
| Compiler | <-- | Build    | <-- | Lock     |     | Package  |
|          |     | Engine   |     | File     |     | Cache    |
+----------+     +----------+     +----------+     +----------+
```

---

## 3. CLI Design & Commands

### 3.1 Command Reference

#### 3.1.1 `nu new` — Create New Project

```bash
# Create a new binary (executable) project
$ nu new my-app
    Created binary (application) `my-app` package
    my-app/
    ├── Nulang.toml
    ├── src/
    │   └── main.nl
    └── tests/
        └── main_test.nl

# Create a new library project
$ nu new my-library --lib
    Created library `my-library` package
    my-library/
    ├── Nulang.toml
    ├── src/
    │   └── lib.nl
    └── tests/
        └── lib_test.nl

# Create a workspace project
$ nu new my-monorepo --workspace
    Created workspace `my-monorepo`
    my-monorepo/
    ├── Nulang.toml
    ├── packages/
    └── src/

# Create with specific edition
$ nu new my-app --edition 2024

# Create in existing directory
$ nu init --lib
```

#### 3.1.2 `nu build` — Build Project

```bash
# Debug build (default)
$ nu build
   Compiling my-app v0.1.0 (/home/dev/my-app)
    Finished `dev` profile [unoptimized] target(s) in 1.23s

# Release build
$ nu build --release
   Compiling my-app v0.1.0 (/home/dev/my-app)
    Finished `release` profile [optimized] target(s) in 8.45s

# Build specific package in workspace
$ nu build -p http-client

# Build all packages
$ nu build --workspace

# Build with specific features
$ nu build --features "json compression"
$ nu build --all-features
$ nu build --no-default-features

# Build specific target
$ nu build --target wasm32-unknown-nulang
$ nu build --target x86_64-unknown-linux-gnu

# Incremental build (default in dev)
$ nu build --incremental

# Verbose output
$ nu build -v
$ nu build -vv  # Very verbose

# Dry run (show what would be built)
$ nu build --dry-run

# JSON output for tooling integration
$ nu build --message-format=json
```

#### 3.1.3 `nu test` — Run Tests

```bash
# Run all tests
$ nu test
   Compiling my-app v0.1.0
    Running 15 tests

test unit::math::add ... ok
test unit::math::subtract ... ok
test unit::string::format ... ok
test integration::api::get_users ... ok
test integration::api::create_user ... ok

test result: ok. 15 passed; 0 failed; 0 ignored

# Run specific test
$ nu test math::add
$ nu test --test unit_tests

# Run with filter
$ nu test --filter "user"

# Run ignored tests
$ nu test --ignored
$ nu test --include-ignored

# Run with all features
$ nu test --all-features

# Run with coverage
$ nu test --coverage
   Coverage: 87.3% (142/163 lines)
   Missing: src/auth.nl:45-52, src/db.nl:12-15

# Run benchmarks
$ nu bench
$ nu bench --filter "sorting"

# Run in release mode
$ nu test --release

# Number of parallel jobs
$ nu test -j 8

# Show test output
$ nu test --nocapture

# Watch mode
$ nu test --watch
```

#### 3.1.4 `nu add` — Add Dependencies

```bash
# Add a dependency from the registry
$ nu add http
    Adding http ^3.2.0 to dependencies

# Add with version constraint
$ nu add http@"^2.0"
$ nu add http@">=1.0, <3.0"
$ nu add http@"=1.5.2"

# Add a dev dependency
$ nu add test-framework --dev

# Add a build dependency
$ nu add protoc --build

# Add from git repository
$ nu add ai-sdk --git https://github.com/nulang/ai-sdk
$ nu add ai-sdk --git https://github.com/nulang/ai-sdk --branch main
$ nu add ai-sdk --git https://github.com/nulang/ai-sdk --tag v1.0.0
$ nu add ai-sdk --git https://github.com/nulang/ai-sdk --rev abc1234

# Add from local path
$ nu add database --path ../database

# Add with features
$ nu add tls --features "rustls"
$ nu add tls --features "rustls,http2"
$ nu add tls --all-features

# Add and update lock file
$ nu add http --update

# Remove a dependency
$ nu remove http
$ nu rm http

# Update dependencies
$ nu update                    # Update all
$ nu update http               # Update specific package
$ nu update http@"^3.0"        # Update with new constraint
```

#### 3.1.5 `nu run` — Run Project

```bash
# Run the main binary
$ nu run
   Compiling my-app v0.1.0
    Running `target/debug/my-app`
Hello, World!

# Run with arguments
$ nu run -- --port 8080 --verbose

# Run in release mode
$ nu run --release

# Run specific binary
$ nu run --bin my-app
$ nu run --bin cli-tool

# Run example
$ nu run --example basic

# Run tests continuously
$ nu run --watch

# Environment variables
$ NU_LOG=debug nu run
```

#### 3.1.6 `nu publish` — Publish Package

```bash
# Publish to registry
$ nu publish
    Packaging my-library v1.0.0
    Verifying package
    Uploading to registry.nulang.org
    Published my-library v1.0.0

# Dry run (verify without publishing)
$ nu publish --dry-run

# Publish to alternative registry
$ nu publish --registry https://internal.registry.company.com

# Publish with specific token
$ nu publish --token $NULANG_REGISTRY_TOKEN

# Allow dirty working directory
$ nu publish --allow-dirty

# Skip verification
$ nu publish --no-verify
```

#### 3.1.7 `nu search` — Search Packages

```bash
# Search for packages
$ nu search http
    http (3.2.0) - High-performance HTTP client/server
    http-client (2.1.0) - HTTP client library
    http-server (1.5.0) - HTTP server framework

# Detailed search
$ nu search http --detailed
    http v3.2.0
    =============
    HTTP client and server implementation
    Downloads: 1.2M | Stars: 450 | License: MIT
    https://nulang.org/packages/http

# Search with filters
$ nu search json --min-downloads 10000
$ nu search web --category networking

# List versions
$ nu show http
    http = "3.2.0"
        Features: json, compression, http2, websocket
        Dependencies: async-runtime, tls, serde

    http = "3.1.0" ...
    http = "3.0.0" ...

# Show full manifest
$ nu show http --manifest
```

#### 3.1.8 `nu doc` — Documentation

```bash
# Generate documentation
$ nu doc
 Documenting my-app v0.1.0
    Finished docs for 15 modules

# Open in browser
$ nu doc --open

# Generate for dependencies too
$ nu doc --document-private-items

# Serve docs locally
$ nu doc --serve --port 3000

# Check documentation coverage
$ nu doc --check-coverage
   Missing docs: 3 public items
   - src/auth.nl:fn authenticate/2
   - src/db.nl:type ConnectionConfig
   - src/http.nl:module middleware
```

#### 3.1.9 `nu check` — Static Analysis

```bash
# Type check
$ nu check
    Checking my-app v0.1.0
    Finished: 0 errors, 2 warnings

# With warnings as errors
$ nu check --deny-warnings

# Check specific package
$ nu check -p http-client

# Format check
$ nu fmt --check

# Run linter
$ nu lint
   Checking style...
   Checking common mistakes...
   Checking security...

# Security audit
$ nu audit
   Scanning 25 dependencies...
   No known vulnerabilities found.

$ nu audit --json
```

#### 3.1.10 `nu clean` — Clean Build Artifacts

```bash
# Clean build directory
$ nu clean
   Removed target/debug/
   Removed target/release/

# Clean everything including cache
$ nu clean --all
   Removed target/
   Removed ~/.nulang/cache/

# Clean specific package
$ nu clean -p http-client
```

### 3.2 Global Options

```bash
# Verbose output
$ nu -v <command>
$ nu -vv <command>  # Very verbose

# Quiet mode
$ nu -q <command>

# Specify manifest
$ nu --manifest path/to/Nulang.toml build

# Color output control
$ nu --color always test
$ nu --color never test
$ nu --color auto test   # Default

# JSON output
$ nu --json test

# No progress bars
$ nu --no-progress build

# Specify jobs (parallelism)
$ nu -j 16 build

# Specify target directory
$ nu --target-dir /tmp/build build

# Offline mode (use only cached packages)
$ nu --offline build

# Show version
$ nu --version
nu 1.0.0 (2024-06-15)

# Show help
$ nu --help
$ nu <command> --help
```

---

## 4. API Design & Specification

### 4.1 Manifest Format (Nulang.toml)

#### 4.1.1 Package Section

```toml
[package]
# Required fields
name = "my-package"
version = "1.0.0"
edition = "2024"

# Optional metadata
authors = ["Author Name <email@example.com>"]
description = "A short description of the package"
documentation = "https://docs.example.com/my-package"
homepage = "https://example.com/my-package"
repository = "https://github.com/username/my-package"
license = "MIT"                          # SPDX identifier
license-file = "LICENSE"                 # Alternative to 'license'
readme = "README.md"
keywords = ["web", "http", "async"]      # Up to 5
categories = ["network-programming"]     # From approved list

# Build configuration
type = "lib"                             # "lib" | "bin" | "cdylib" | "staticlib"
autoexamples = true
autotests = true
autobenches = true
build = "build.nl"                       # Custom build script
links = "native-lib"                     # Link to native library

# Exclusion patterns
exclude = ["fixtures/**/*", "*.log"]
include = ["src/**", "README.md"]

# Rust-style features
[features]
default = ["std", "json"]
std = []
json = ["serde"]
full = ["std", "json", "compression"]

# Build profiles
[profile.dev]
opt_level = 0
debug = true

[profile.release]
opt_level = 3
lto = "fat"
```

#### 4.1.2 Dependencies Section

```toml
[dependencies]
# Simple version constraint
http = "^1.2.0"

# Full specification
async-runtime = { 
  version = "^2.0",
  features = ["tokio", "metrics"],
  optional = false,
  default-features = false,
  registry = "nulang",
  target = "cfg(unix)"
}

# Git dependency
database = { 
  git = "https://github.com/nulang/database",
  branch = "main",
  # or: tag = "v1.0.0"
  # or: rev = "abc1234"
}

# Path dependency (local)
internal = { path = "../internal" }

# Platform-specific
tls-native = { version = "1.0", target = "cfg(unix)" }
tls-windows = { version = "1.0", target = "cfg(windows)" }

# Development dependencies
[dev-dependencies]
test-harness = "^1.0"
mock-framework = "^2.0"
fuzz-target = { version = "1.0", optional = true }

# Build dependencies (for build scripts)
[build-dependencies]
protoc = "^3.0"
```

#### 4.1.3 Workspace Section

```toml
[workspace]
# Members (glob patterns supported)
members = ["packages/*", "tools/*", "apps/*"]

# Exclude from workspace
exclude = ["experiments", "deprecated/*"]

# Shared metadata
package.version = "1.0.0"
package.edition = "2024"
package.authors = ["Team <team@example.com>"]
package.license = "MIT"
package.repository = "https://github.com/example/monorepo"

# Shared dependencies
[workspace.dependencies]
async-runtime = "2.1.0"
http = "3.0.0"
database = { path = "packages/database" }
serde = "1.0.200"

# In member Nulang.toml:
[package]
name = "http-client"
version.workspace = true
edition.workspace = true

[dependencies]
async-runtime = { workspace = true }
http = { workspace = true, features = ["server"] }
```

### 4.2 Version Constraint Grammar

```nulang
// Version constraint syntax
"1.2.3"       // Exact version =1.2.3
"^1.2.3"      // Compatible: >=1.2.3, <2.0.0
"~1.2.3"      // Approximately: >=1.2.3, <1.3.0
">=1.2.3"     // Greater than or equal
">1.2.3"      // Greater than
"<2.0.0"      // Less than
"<=2.0.0"     // Less than or equal
">=1.0, <2.0" // Range
"*"           // Any version
"1.x"         // Wildcard: >=1.0.0, <2.0.0
"1.2.*"       // Wildcard: >=1.2.0, <1.3.0
"latest"      // Latest version
"branch:main" // Git branch
"tag:v1.0.0"  // Git tag
"rev:abc1234" // Git revision
```

### 4.3 Lock File Format (Nulang.lock)

```toml
# This file is automatically @generated by Nulang.
# It is not intended for manual editing.
version = 4

[[package]]
name = "my-app"
version = "0.1.0"
dependencies = [
  "http 3.2.0",
  "json 2.1.0",
]

[[package]]
name = "http"
version = "3.2.0"
source = "registry+https://registry.nulang.org/api/v1"
checksum = "sha256:a1b2c3d4e5f6..."
dependencies = [
  "async-runtime 2.1.0",
  "tls 1.3.0",
]

[[package]]
name = "json"
version = "2.1.0"
source = "registry+https://registry.nulang.org/api/v1"
checksum = "sha256:f6e5d4c3b2a1..."
dependencies = [
  "serde 3.0.0",
]

[[package]]
name = "async-runtime"
version = "2.1.0"
source = "registry+https://registry.nulang.org/api/v1"
checksum = "sha256:1a2b3c4d5e6f..."
dependencies = []

[[package]]
name = "serde"
version = "3.0.0"
source = "registry+https://registry.nulang.org/api/v1"
checksum = "sha256:abcdef123456..."
dependencies = []

[[package]]
name = "tls"
version = "1.3.0"
source = "registry+https://registry.nulang.org/api/v1"
checksum = "sha256:fedcba654321..."
dependencies = []
features = ["rustls"]
```

### 4.4 Build Script API

```nulang
// build.nl — Custom build script
use nu::build;

fn main() {
  // Rerun if these files change
  build::rerun_if_changed("src/schema.proto");
  build::rerun_if_changed("build.nl");
  
  // Set compile-time environment variables
  build::set_env("GIT_COMMIT", build::command("git", ["rev-parse", "HEAD"]));
  build::set_env("BUILD_DATE", build::command("date", ["-u", "+%Y-%m-%dT%H:%M:%SZ"]));
  
  // Compile protobuf definitions
  build::command_or_fail("protoc", [
    "--nulang_out=src/generated",
    "--proto_path=src",
    "src/schema.proto"
  ]);
  
  // Generate code
  let out_dir = build::env("OUT_DIR");
  generate_version_file("{out_dir}/version.nl");
  
  // Link to native library
  build::link_lib("ssl");
  build::link_search("/usr/local/lib");
  
  // Conditional compilation flags
  if build::target().os == "macos" {
    build::set_flag("macos");
    build::link_framework("CoreFoundation");
  }
  
  // Download/generate assets
  if !build::path_exists("src/assets.json") {
    build::write_file("src/assets.json", generate_asset_manifest());
  }
}

fn generate_version_file(path: String) {
  let version = build::env("CARGO_PKG_VERSION");
  let commit = build::env("GIT_COMMIT");
  
  build::write_file(path, """
    // Auto-generated by build script
    pub const VERSION = "{version}";
    pub const GIT_COMMIT = "{commit}";
  """)
}
```

### 4.5 Package Configuration API

```nulang
// Programmatic access to package configuration
use nu::manifest;

fn read_manifest() {
  // Parse Nulang.toml
  let manifest = manifest::parse("Nulang.toml")
    |> expect("Failed to parse manifest");
  
  // Access package info
  println("Name: {manifest.package.name}");
  println("Version: {manifest.package.version}");
  println("Edition: {manifest.package.edition}");
  
  // Iterate dependencies
  for dep in manifest.dependencies {
    match dep.source {
      Registry(version) => println("  {dep.name} @ {version}"),
      Git(url, ref) => println("  {dep.name} @ git:{url}#{ref}"),
      Path(p) => println("  {dep.name} @ path:{p}")
    }
  }
  
  // Check features
  if manifest.features.enabled.contains("json") {
    println("JSON support enabled");
  }
  
  // Access scripts
  if let Some(test_script) = manifest.scripts.get("test") {
    println("Test command: {test_script}");
  }
}
```

---

## 5. Module Reference

### 5.1 Module Hierarchy

```
nu/
├── cli/
│   ├── main.nl           # CLI entry point
│   ├── commands/
│   │   ├── new.nl        # `nu new` command
│   │   ├── build.nl      # `nu build` command
│   │   ├── test.nl       # `nu test` command
│   │   ├── add.nl        # `nu add` command
│   │   ├── remove.nl     # `nu remove` command
│   │   ├── run.nl        # `nu run` command
│   │   ├── publish.nl    # `nu publish` command
│   │   ├── search.nl     # `nu search` command
│   │   ├── doc.nl        # `nu doc` command
│   │   ├── check.nl      # `nu check` command
│   │   ├── clean.nl      # `nu clean` command
│   │   └── fmt.nl        # `nu fmt` command
│   ├── args.nl           # Argument parsing
│   ├── output.nl         # Terminal output formatting
│   └── progress.nl       # Progress bars
├── core/
│   ├── manifest.nl       # Nulang.toml parser
│   ├── lockfile.nl       # Nulang.lock parser/writer
│   ├── package.nl        # Package representation
│   ├── project.nl        # Project discovery
│   └── workspace.nl      # Workspace management
├── resolver/
│   ├── sat_solver.nl     # SAT-based resolution
│   ├── version.nl        # Version parsing/comparison
│   ├── constraints.nl    # Constraint satisfaction
│   ├── conflict.nl       # Conflict detection/reporting
│   └── graph.nl          # Dependency graph
├── registry/
│   ├── client.nl         # HTTP client for registry
│   ├── auth.nl           # Authentication
│   ├── download.nl       # Package download
│   ├── upload.nl         # Package upload
│   ├── search.nl         # Search API
│   └── cache.nl          # Local package cache
├── build/
│   ├── compiler.nl       # Compiler interface
│   ├── linker.nl         # Linker interface
│   ├── cache.nl          # Build artifact cache
│   ├── parallel.nl       # Parallel job execution
│   ├── profiles.nl       # Build profiles
│   └── targets.nl        # Target platform management
├── test/
│   ├── runner.nl         # Test runner
│   ├── discovery.nl      # Test discovery
│   ├── harness.nl        # Test harness
│   ├── reporter.nl       # Test result reporter
│   ├── coverage.nl       # Coverage collection
│   └── snapshot.nl       # Snapshot testing
├── quality/
│   ├── linter.nl         # Linting engine
│   ├── formatter.nl      # Code formatter
│   ├── audit.nl          # Security auditing
│   └── doc_gen.nl        # Documentation generation
└── util/
    ├── toml.nl           # TOML parsing
    ├── semver.nl         # Semantic versioning
    ├── hash.nl           # Hashing utilities
    ├── archive.nl        # Archive (tar/zip) handling
    └── fs.nl             # Filesystem utilities
```

### 5.2 Core Types

```nulang
// Package manifest
type Manifest = {
  package: PackageMetadata,
  dependencies: [Dependency],
  dev_dependencies: [Dependency],
  build_dependencies: [Dependency],
  features: Map<String, [String]>,
  profiles: Map<String, Profile>,
  scripts: Map<String, String>,
  workspace: Option<WorkspaceConfig>,
  registry: RegistryConfig
}

type PackageMetadata = {
  name: String,
  version: Version,
  edition: Edition,
  authors: [String],
  description: Option<String>,
  documentation: Option<String>,
  repository: Option<String>,
  license: Option<String>,
  license_file: Option<String>,
  readme: Option<String>,
  keywords: [String],
  categories: [String],
  type: PackageType,
  build: Option<String>,
  links: Option<String>,
  exclude: [String],
  include: [String],
  autobins: Bool,
  autoexamples: Bool,
  autotests: Bool,
  autobenches: Bool
}

enum PackageType {
  Library,
  Binary,
  CDyLib,       // C-compatible dynamic library
  StaticLib     // Static library
}

type Dependency = {
  name: String,
  source: DependencySource,
  features: [String],
  optional: Bool,
  default_features: Bool,
  target: Option<String>,     // Platform condition
  registry: Option<String>
}

enum DependencySource {
  Registry(VersionConstraint),
  Git { url: String, reference: GitRef },
  Path(String)
}

enum GitRef {
  Branch(String),
  Tag(String),
  Rev(String)
}

type VersionConstraint =
  | Exact(Version)
  | Caret(Version)       // ^1.2.3
  | Tilde(Version)       // ~1.2.3
  | GreaterThan(Version)
  | GreaterThanEq(Version)
  | LessThan(Version)
  | LessThanEq(Version)
  | Range(VersionConstraint, VersionConstraint)
  | WildcardMajor(Int)   // 1.*
  | WildcardMinor(Int, Int) // 1.2.*
  | Any

type Version = {
  major: Int,
  minor: Int,
  patch: Int,
  pre: [PrereleaseIdentifier],
  build: [String]
}

type WorkspaceConfig = {
  members: [String],        // Glob patterns
  exclude: [String],
  default_members: [String],
  resolver: String,
  package: Option<WorkspacePackageDefaults>,
  dependencies: Option<Map<String, DependencySource>>
}

type Profile = {
  opt_level: OptLevel,
  debug: Bool,
  split_debug_info: Option<String>,
  debug_assertions: Bool,
  overflow_checks: Bool,
  lto: LtoSetting,
  panic: PanicStrategy,
  incremental: Bool,
  codegen_units: Option<Int>,
  rpath: Bool,
  strip: StripSetting
}

enum OptLevel {
  None,
  Basic,
  Aggressive,
  Size,
  SizeAggressive
}

enum LtoSetting {
  None,
  Thin,
  Fat,
  Bool(Bool)
}

enum PanicStrategy {
  Unwind,
  Abort
}

enum StripSetting {
  None,
  DebugInfo,
  Symbols
}
```

---

## 6. Implementation Phases

### 6.1 Phase 1: Core CLI & Manifest (Weeks 1-4)

**Goal:** Build the CLI framework, manifest parser, and project scaffolding.

```
Milestone: v0.1.0 — "Bootstrap"
+---------------------------------------------------------------+
| Week 1-2            | Week 3-4                                |
+---------------------+-----------------------------------------+
| CLI framework       | Manifest parser                         |
|                     |                                         |
| - Command routing   | - TOML parsing                          |
| - Argument parsing  | - Manifest validation                   |
| - Help generation   | - Default value handling                |
| - Error formatting  | - Edition validation                    |
| - Progress bars     |                                         |
|                     | Project scaffolding                     |
| Terminal output     | - Template generation                   |
| - Colors            | - File creation                         |
| - Tables            | - Git init                              |
| - Spinners          | - README/license templates              |
+---------------------+-----------------------------------------+
| Deliverable: nu new, nu --version, nu --help working          |
| Tests: CLI argument parsing, project template generation      |
+---------------------------------------------------------------+
```

**Key Tasks:**
- [ ] Build CLI argument parser
- [ ] Implement command routing
- [ ] Create TOML manifest parser
- [ ] Build manifest validation
- [ ] Implement project scaffolding (nu new)
- [ ] Create project templates (lib, bin, workspace)
- [ ] Add terminal output formatting
- [ ] Write comprehensive tests

### 6.2 Phase 2: Dependency Resolution (Weeks 5-8)

**Goal:** Build the dependency resolver and lock file system.

```
Milestone: v0.2.0 — "Resolve"
+---------------------------------------------------------------+
| Week 5-6            | Week 7-8                                |
+---------------------+-----------------------------------------+
| Version system      | SAT solver                              |
|                     |                                         |
| - SemVer parsing    | - Constraint encoding                   |
| - Version compare   | - Backtracking search                   |
| - Constraint parse  | - Conflict detection                    |
|                     | - Conflict explanation                  |
| Registry client     | Lock file                               |
| - HTTP client       |                                         |
| - Package download  | - Lock file generation                  |
| - Local cache       | - Lock file validation                  |
| - Index update      | - Lock file update (conservative)       |
+---------------------+-----------------------------------------+
| Deliverable: nu add working with lock file generation         |
| Tests: Resolver unit tests, integration with mock registry    |
+---------------------------------------------------------------+
```

**Key Tasks:**
- [ ] Implement SemVer parsing and comparison
- [ ] Build version constraint parser
- [ ] Create registry HTTP client
- [ ] Implement package download with caching
- [ ] Build SAT-based dependency resolver
- [ ] Implement conflict detection and reporting
- [ ] Create lock file parser and writer
- [ ] Implement lock file validation
- [ ] Write comprehensive tests

### 6.3 Phase 3: Build System (Weeks 9-14)

**Goal:** Build the compilation and linking pipeline.

```
Milestone: v0.3.0 — "Build"
+---------------------------------------------------------------+
| Week 9-10           | Week 11-12         | Week 13-14         |
+---------------------+--------------------+--------------------+
| Compiler interface  | Build orchestration | Incremental builds |
|                     |                    |                    |
| - AST parsing       | - Parallel jobs    | - File hashing     |
| - Type checking     | - Job scheduling   | - Change detection |
| - IR generation     | - Profile support  | - Selective rebuild|
| - Code generation   | - Target platform  | - Artifact caching |
|                     |                    |                    |
| Linker interface    | Workspace builds   | Build scripts      |
|