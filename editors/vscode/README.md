# Nulang for VS Code

Language support for [Nulang](https://github.com/nulang-org/nulang) in Visual Studio Code: syntax highlighting, language essentials (comments, brackets, indentation, folding), and snippets.

## Features

- **Syntax highlighting** for `.nula` files via a TextMate grammar covering:
  - Keywords: `fn`, `let`, `const`, `type`, `alias`, `effect`, `actor`, `behavior`, `state`, `spawn`, `send`, `receive`, `handle`, `perform`, `resume`, `match`, `case`, `if`/`else`, `for`, `while`, `loop`, `return`, `import`, `pub`, `extern`, and more
  - Reference capabilities: `iso`, `trn`, `ref`, `val`, `box`, `tag`, `lineariso` (including `@cap` annotations)
  - Primitive and standard types, effect names (`IO`, `Http`, `Json`, `LLM`, ...), user-defined types
  - Strings with escape sequences, character literals, comments (`//` and `/* */`), numbers (int, float, hex, binary, octal), and operators (`->`, `=>`, `|>`, `!`, `<-`, `..`, ...)
  - Declaration highlighting: function, actor, behavior, type, and effect names
- **Language configuration**: comment toggling (line + block), bracket matching, auto-closing pairs, indentation rules, and region folding (`// #region` / `// #endregion`)
- **Snippets** for common forms: `fn`, `actor`, `behavior`, `spawn`, `send`, `handle`, `match`, `effect`, loops, and more

## Installation

### From a `.vsix`

```sh
cd editors/vscode
npx @vscode/vsce package
code --install-extension nulang-0.1.0.vsix
```

### Manual (development)

Symlink or copy this directory into your VS Code extensions folder:

```sh
ln -s "$PWD/editors/vscode" ~/.vscode/extensions/nulang
```

Then reload VS Code.

## Usage

Open any `.nula` file — the grammar activates automatically. Try the
[examples](https://github.com/nulang-org/nulang/tree/main/examples) in the main
repository.

## Scope

This extension provides TextMate-based highlighting and language essentials
only. A language server (diagnostics, hover, goto-definition) is planned
separately and is not part of this package.

## License

Apache-2.0, same as the Nulang repository. See [LICENSE](./LICENSE).
