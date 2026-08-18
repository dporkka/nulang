# Nulang VS Code Extension

Syntax highlighting, diagnostics, and LSP features for the [Nulang](https://nulang.org) programming language.

## Features

- **Syntax highlighting** — keywords, types, strings, numbers, comments
- **Diagnostics** — parse errors, type errors, effect errors on save
- **Go-to-definition** — jump to function/type/variable definitions
- **References** — find all usages of a symbol
- **Hover** — type information on hover
- **Completion** — keyword and built-in effect suggestions
- **Rename** — project-wide symbol rename
- **Formatting** — indentation support

## Installation

### From .vsix (recommended)

```bash
# Build from source (requires Node.js):
cd .vscode
npm install
npx @vscode/vsce package

# Install:
code --install-extension nulang-0.1.0.vsix
```

### Manual install

```bash
cp -r .vscode ~/.vscode/extensions/nulang/
```

### From the Marketplace

Search for "Nulang" in the VS Code Extensions view (coming soon).

## Requirements

- **Nulang compiler** installed and on your PATH (or set `NULANG_PATH` env var)
- Build from source: `git clone https://github.com/nulang-org/nulang && cd nulang && cargo build --release`

## Configuration

Set `NULANG_PATH` to the full path of the `nulang` binary if it's not on your PATH:

```json
{
  "nulang.serverPath": "/path/to/nulang"
}
```

## Commands

- **Nulang: Restart Language Server** — restart the LSP server after updating the compiler

## Development

The extension is minimal: `extension.js` launches `nulang --lsp` as a stdio language server.
The LSP server (`src/lsp/mod.rs`) provides 12 features including hover, goto-definition, references,
rename, signature help, inlay hints, completion, and diagnostics.

## License

Apache-2.0
