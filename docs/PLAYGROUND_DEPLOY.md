# Deploying the Nulang Playground to nulang.org/playground

The browser playground (`playground/web/`) is a **static bundle**: four
files, no backend. Anything that can serve static files with the right MIME
types can host it.

## 1. Build the bundle

```bash
rustup target add wasm32-unknown-unknown   # one-time
playground/web/build.sh
```

This compiles `crates/nulang-playground` for `wasm32-unknown-unknown` and
copies the artifact into `playground/web/`:

```
playground/web/
├── index.html               # editor UI
├── playground.js            # wasm loader + run driver (no frameworks)
├── style.css
└── nulang_playground.wasm   # build artifact (~1.2 MB, git-ignored)
```

Sanity check locally:

```bash
cd playground/web && python3 -m http.server 8080
# open http://localhost:8080, press Run
```

## 2. Serve it at nulang.org/playground

The bundle uses **relative paths only**, so it works under any sub-path.
Copy the four files to whatever directory the nulang.org web server maps to
`/playground/`, e.g.:

```bash
rsync -av playground/web/ webroot/playground/
```

### Required response headers / MIME types

| File                     | Content-Type               | Notes                          |
|--------------------------|----------------------------|--------------------------------|
| `*.wasm`                 | `application/wasm`         | **required** for fast compile  |
| `*.js`                   | `text/javascript`          |                                |
| everything               | `Cache-Control` short-ish  | wasm is content-stable per build |

`playground.js` uses `fetch()` + `WebAssembly.instantiate()` (not
`instantiateStreaming`), so a wrong wasm MIME type degrades to a warning,
not a failure — but set `application/wasm` anyway.

**nginx** (recent versions already map `.wasm` → `application/wasm`):

```nginx
location /playground/ {
    alias /var/www/nulang.org/playground/;
    types { application/wasm wasm; }   # only if your mime.types lacks it
    add_header Cache-Control "public, max-age=300";
}
```

**Caddy**: nothing to do; Caddy serves `application/wasm` out of the box.

### Static-hosting alternatives (no server access needed)

- **GitHub Pages**: commit the bundle to a `gh-pages` branch (the `.wasm`
  must be force-added since it is git-ignored in `main`: `git add -f`), or
  publish from a release artifact via CI.
- **Cloudflare Pages / Netlify / Vercel**: point the site at a build that
  runs `playground/web/build.sh` (Rust toolchain available in all three) and
  publish `playground/web/` as the output directory, or upload the four
  files directly.
- **S3 + CloudFront**: upload with
  `aws s3 cp playground/web/ s3://bucket/playground/ --recursive` and set
  `--content-type application/wasm` on the `.wasm` object.

### CI sketch (GitHub Actions)

```yaml
- uses: dtolnay/rust-toolchain@stable
  with:
    targets: wasm32-unknown-unknown
- run: playground/web/build.sh
- uses: actions/upload-artifact@v4
  with:
    name: playground
    path: playground/web/
# then publish `playground/web/` with your pages/deploy step of choice
```

## 3. Verify after deploy

1. `curl -sI https://nulang.org/playground/nulang_playground.wasm` →
   `200` and `content-type: application/wasm`.
2. Open `https://nulang.org/playground/` — the status line should read
   **"compiler ready"**.
3. Press **Run** on the default example — output should be:
   ```
   Hello, Nulang!
   Hello, World!
   The answer is 42
   ```

## Notes

- The wasm is the real compiler front-end + CoreVM (`src/core_vm`), built
  from the same sources as the native binary. Language support == the
  `core-vm` backend: frozen Core subset, `IO.print`, closures, recursion.
  Actors, networking, FFI, JIT are native-only by design.
- The old server-side playground (`playground/server.py`) remains available
  for features that need the full native runtime.
