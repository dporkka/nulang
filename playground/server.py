#!/usr/bin/env python3
"""Nulang Playground Server — serves index.html and runs Nulang code via POST /run, /compile, /check.

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
        Path(__file__).resolve().parent.parent / "target" / "debug" / "nulang",
        Path(__file__).resolve().parent.parent / "target" / "release" / "nulang",
    ]
    for c in candidates:
        if c.exists():
            NULANG_BIN = str(c)
            break
    if not NULANG_BIN:
        # Try to find in PATH
        import shutil
        NULANG_BIN = shutil.which("nulang") or "nulang"


class Handler(http.server.BaseHTTPRequestHandler):
    def send_json(self, data):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(data).encode())

    def send_error_response(self, data):
        self.send_response(400)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(json.dumps(data).encode())

    def do_GET(self):
        if self.path == "/" or self.path == "/index.html":
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.end_headers()
            self.wfile.write(INDEX_HTML.encode())
        else:
            self.send_response(404)
            self.end_headers()

    def do_POST(self):
        if self.path == "/run":
            self.handle_run()
        elif self.path == "/compile":
            self.handle_compile()
        elif self.path == "/check":
            self.handle_check()
        else:
            self.send_response(404)
            self.end_headers()

    def handle_run(self):
        content_length = int(self.headers.get('Content-Length', 0))
        body = self.rfile.read(content_length).decode('utf-8')
        try:
            data = json.loads(body)
            code = data.get('code', '')
        except:
            self.send_error_response({'ok': False, 'stderr': 'Invalid JSON'})
            return

        try:
            # Write code to temp file
            import tempfile
            with tempfile.NamedTemporaryFile(mode='w', suffix='.nula', delete=False) as f:
                f.write(code)
                temp_file = f.name

            result = subprocess.run(
                [NULANG_BIN, '--backend', 'native', temp_file],
                capture_output=True,
                text=True,
                timeout=30
            )
            
            os.unlink(temp_file)
            
            self.send_json({
                'ok': result.returncode == 0,
                'stdout': result.stdout,
                'stderr': result.stderr
            })
        except subprocess.TimeoutExpired:
            self.send_json({'ok': False, 'stderr': 'Execution timed out (30s)'})
        except Exception as e:
            self.send_json({'ok': False, 'stderr': str(e)})

    def handle_compile(self):
        content_length = int(self.headers.get('Content-Length', 0))
        body = self.rfile.read(content_length).decode('utf-8')
        try:
            data = json.loads(body)
            code = data.get('code', '')
            target = data.get('target', 'native')
        except:
            self.send_error_response({'ok': False, 'stderr': 'Invalid JSON'})
            return

        try:
            import tempfile
            with tempfile.NamedTemporaryFile(mode='w', suffix='.nula', delete=False) as f:
                f.write(code)
                temp_file = f.name

            result = subprocess.run(
                [NULANG_BIN, '--backend', 'native', '--target', target, '--emit-nbc', temp_file],
                capture_output=True,
                text=True,
                timeout=30
            )
            
            os.unlink(temp_file)
            
            self.send_json({
                'ok': result.returncode == 0,
                'stdout': result.stdout,
                'stderr': result.stderr
            })
        except subprocess.TimeoutExpired:
            self.send_json({'ok': False, 'stderr': 'Compilation timed out (30s)'})
        except Exception as e:
            self.send_json({'ok': False, 'stderr': str(e)})

    def handle_check(self):
        content_length = int(self.headers.get('Content-Length', 0))
        body = self.rfile.read(content_length).decode('utf-8')
        try:
            data = json.loads(body)
            code = data.get('code', '')
        except:
            self.send_error_response({'ok': False, 'stderr': 'Invalid JSON'})
            return

        try:
            import tempfile
            with tempfile.NamedTemporaryFile(mode='w', suffix='.nula', delete=False) as f:
                f.write(code)
                temp_file = f.name

            result = subprocess.run(
                [NULANG_BIN, '--check', temp_file],
                capture_output=True,
                text=True,
                timeout=30
            )
            
            os.unlink(temp_file)
            
            self.send_json({
                'ok': result.returncode == 0,
                'stdout': result.stdout,
                'stderr': result.stderr
            })
        except subprocess.TimeoutExpired:
            self.send_json({'ok': False, 'stderr': 'Type check timed out (30s)'})
        except Exception as e:
            self.send_json({'ok': False, 'stderr': str(e)})

    def log_message(self, format, *args):
        sys.stderr.write("[playground] %s\n" % (format % args))


if __name__ == "__main__":
    import argparse
    parser = argparse.ArgumentParser()
    parser.add_argument('--port', type=int, default=8080)
    parser.add_argument('--nulang', type=str, default=None)
    args = parser.parse_args()
    
    if args.nulang:
        NULANG_BIN = args.nulang
    
    port = args.port
    server = http.server.HTTPServer(('', port), Handler)
    print(f"Nulang Playground running on http://localhost:{port}")
    print(f"Using nulang binary: {NULANG_BIN}")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nShutting down...")
        server.server_close()
