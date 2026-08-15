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

## Hosted version

A hosted playground at [nulang.org/playground](https://nulang.org/playground)
is **coming soon** — for now, run it locally as above.
