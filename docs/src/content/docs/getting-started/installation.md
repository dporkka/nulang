---
title: Installation
description: Build Nulang from source and set up your development environment.
---

## Prerequisites

- **Rust** stable 1.93+ ([rustup](https://rustup.rs))
- **Python 3** development headers (for PyO3 interop)
- **Linux** or **macOS** (Windows planned)
- A C compiler and linker (GCC/Clang + GNU `bfd` linker on Linux)

## Building from Source

```bash
git clone https://github.com/dporkka/nulang.git
cd nulang
cargo build --release
```

The release build uses `opt-level=3`, LTO, and single codegen-unit for maximum performance. A debug build (`cargo build`) is faster to compile but runs ~10x slower.

### Feature Flags

Nulang supports optional features for leaner builds:

```bash
# Minimal build (no Python, SQLite, or LSP)
cargo build --release --no-default-features

# With WASM backend
cargo build --release --features wasm-backend

# All features
cargo build --release --all-features
```

| Feature | Default | Description |
|---------|---------|-------------|
| `python` | On | PyO3 Python interop |
| `sqlite` | On | libsql/Turso persistence |
| `lsp` | On | tower-lsp language server |
| `wasm-backend` | Off | WASM compiler + Wasmtime runtime |

## Verifying the Build

```bash
# Run the test suite
cargo test

# Start the REPL
cargo run -- --repl

# Run a Nulang program
cargo run -- examples/hello.nula
```

## System-Specific Notes

### Fedora Linux

Install Python development headers:

```bash
sudo dnf install python3-devel
```

The `build.rs` script automatically creates a `libpythonX.Y.so` symlink for PyO3 linking.

### macOS

Install Python via Homebrew:

```bash
brew install python@3.14
```

## Next Steps

Now that Nulang is installed, follow the [Quick Start](/getting-started/quick-start/) guide to write your first program.
