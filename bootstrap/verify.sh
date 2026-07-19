#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
NULANG="cargo run --quiet --"

echo "=== Bootstrap Core verification ==="

# 1. self_test.nula: fib(10) = 55
result=$($NULANG bootstrap/self_test.nula 2>&1 | tail -1)
if [ "$result" != "55" ]; then
    echo "FAIL: self_test.nula expected 55, got '$result'"
    exit 1
fi
echo "PASS: self_test.nula = 55"

# 2. compiler_core.nula: eval 42+1+3 = 46
result=$($NULANG bootstrap/compiler_core.nula 2>&1 | tail -1)
if [ "$result" != "46" ]; then
    echo "FAIL: compiler_core.nula expected 46, got '$result'"
    exit 1
fi
echo "PASS: compiler_core.nula = 46"

# 3. host.nula: invokes compiler pipeline = 46
result=$($NULANG bootstrap/host.nula 2>&1 | tail -1)
if [ "$result" != "46" ]; then
    echo "FAIL: host.nula expected 46, got '$result'"
    exit 1
fi
echo "PASS: host.nula = 46"

# 4. self_test .nbc round-trip
$NULANG --emit-nbc --out bootstrap/self_test.nbc bootstrap/self_test.nula 2>/dev/null
result=$($NULANG bootstrap/self_test.nbc 2>&1 | tail -1)
if [ "$result" != "55" ]; then
    echo "FAIL: self_test.nbc expected 55, got '$result'"
    exit 1
fi
echo "PASS: self_test.nbc round-trip = 55"
rm -f bootstrap/self_test.nbc

echo ""
echo "=== All bootstrap checks passed ==="
