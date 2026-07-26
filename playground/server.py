#!/usr/bin/env python3
"""Nulang Playground Server — serves index.html and runs Nulang code via POST /run.

Usage:
    python3 playground/server.py [--port 8080] [--nulang ./target/debug/nulang]
"""

import http.server
import json
import os
import subprocess
import sys
import urllib.parse
from pathlib import Path

PLAYGROUND_DIR = Path(__file__).resolve().parent
INDEX_HTML = (PLAYGROUND_DIR / "index.html").read_text()
NULANG_BIN = os.environ.get("NULANG_PATH", None)

# Try to find the nulang binary
if not NULANG_BIN:
    candidates = [
        PLAYGROUND_DIR.parent / "target" / "debug" / "nulang",
        PLAYGROUND_DIR.parent / "target" / "release" / "nulang",
    ]
    for c in candidates:
        if c.exists():
            NULANG_BIN = str(c)
            break
    if not NULANG_BIN:
        NULANG_BIN = "nulang"  # hope it's on PATH


class Handler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/" or self.path == "/index.html":
            self.send_response(200)
            self.send_header("Content-Type", "text/html")
            self.end_headers()
            self.wfile.write(INDEX_HTML.encode())
        else:
            self.send_response(404)
            self.end_headers()

    def do_POST(self):
        if self.path == "/run":
            content_length = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(content_length)
            try:
                data = json.loads(body)
                code = data.get("code", "")
            except json.JSONDecodeError:
                self.send_json({"ok": False, "stderr": "Invalid JSON"})
                return

            try:
                result = subprocess.run(
                    [NULANG_BIN, "--eval", code],
                    capture_output=True,
                    text=True,
                    timeout=10,
                    cwd=str(PLAYGROUND_DIR),
                    env={**os.environ, "NULANG_STEP_LIMIT": "100000"},
                )
                self.send_json({
                    "ok": result.returncode == 0,
                    "stdout": result.stdout,
                    "stderr": result.stderr,
                })
            except subprocess.TimeoutExpired:
                self.send_json({"ok": False, "stderr": "Execution timed out (10s)"})
            except FileNotFoundError:
                self.send_json({
                    "ok": False,
                    "stderr": f"nulang binary not found at '{NULANG_BIN}'. Set NULANG_PATH env var.",
                })
        else:
            self.send_response(404)
            self.end_headers()

    def send_json(self, data):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()
        self.wfile.write(json.dumps(data).encode())

    def log_message(self, format, *args):
        sys.stderr.write("[playground] %s\n" % (format % args))


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 8080
    server = http.server.HTTPServer(("127.0.0.1", port), Handler)
    print(f"Nulang Playground → http://localhost:{port}")
    print(f"  nulang binary: {NULANG_BIN}")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nShutting down.")
        server.server_close()
