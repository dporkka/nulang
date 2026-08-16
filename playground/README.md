# Nulang Playground

A browser playground for trying Nulang without installing anything — edit
`.nula` code, run it, and see compiler output inline.

## Run it locally

Requires a built `nulang` binary (see the repo README) and Python 3:

```bash
cargo build            # or: cargo build --release
python3 playground/server.py
# open http://localhost:8080
```

Options:

```bash
python3 playground/server.py --port 9000 --nulang ./target/release/nulang
```

The server auto-detects `target/debug/nulang` or `target/release/nulang`, or
honors the `NULANG_PATH` environment variable. It serves `index.html` and
exposes `POST /run`, `/compile`, and `/check` endpoints that shell out to the
compiler.

## Browser playground (zero-install, client-side WASM)

`web/` is a **self-contained static bundle**: the real Nulang compiler
front-end (lexer → parser → typechecker → MIR → bytecode) plus the CoreVM
interpreter, compiled to `wasm32-unknown-unknown`. Programs compile and run
entirely in the browser — no server, no install, no network.

Build it (requires `rustup target add wasm32-unknown-unknown`):

```bash
playground/web/build.sh
# then serve statically:
cd playground/web && python3 -m http.server 8080
# open http://localhost:8080
```

`build.sh` compiles `crates/nulang-playground` and copies
`nulang_playground.wasm` into `web/`. The `.wasm` is a build artifact and is
git-ignored; the checked-in files are `index.html`, `playground.js`,
`style.css`, and `build.sh`.

How it works: `crates/nulang-playground` re-uses the compiler's own sources
via `#[path]` includes, so the playground can never drift from the language.
The wasm exports a tiny C-style ABI (`nulang_alloc` / `nulang_run` /
`nulang_free`) — no wasm-bindgen or JS glue generator required. Feature
support matches `nulang run --backend core-vm`: the frozen Core subset,
closures/recursion, `IO.print`; actors, networking, FFI, and the JIT are
native-only. `IO.print` output is captured through `CoreVM::output_sink`.

Deploy to nulang.org: see [`docs/PLAYGROUND_DEPLOY.md`](../docs/PLAYGROUND_DEPLOY.md).

## Hosted version

The hosted playground at [nulang.org/playground](https://nulang.org/playground)
serves the static `web/` bundle above.
