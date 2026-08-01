#!/usr/bin/env bash
# Verify all ```nulang code blocks in documentation.
# Runs each block through --check (parse + type + effect checking).
set -uo pipefail
NULANG="${NULANG_BIN:-cargo run --quiet --}"

DOCS_DIR="docs/src/content/docs"
TMPDIR=$(mktemp -d)
PASS=0
FAIL=0
SKIP=0
TOTAL=0

cleanup() { rm -rf "$TMPDIR"; }
trap cleanup EXIT

# Check if a block looks like a REPL session (starts with nulang>)
is_repl() { echo "$1" | head -1 | grep -q '^nulang>'; }

# Check if block looks runnable (not a declaration fragment)
is_runnable() {
    local first
    first=$(echo "$1" | head -1)
    # Declaration keywords → fragment, not standalone
    echo "$first" | grep -qE '^(actor |behavior |fn |effect |type |workflow|agent |state |import |use |receive )' && return 1
    return 0
}

echo "=== Verifying documentation code examples ==="
echo ""

while IFS= read -r -d '' file; do
    rel="${file#$DOCS_DIR/}"

    # Extract blocks: lines between ```nulang and ```
    in_block=0
    block=""
    block_num=0

    while IFS= read -r line || [[ -n "$line" ]]; do
        if [[ "$line" == '```nulang' ]]; then
            in_block=1
            block=""
            block_num=$((block_num + 1))
            continue
        fi
        if [[ "$line" == '```' ]] && [[ $in_block -eq 1 ]]; then
            in_block=0
            # Process the completed block
            [[ -z "$(echo "$block" | tr -d '[:space:]')" ]] && continue
            TOTAL=$((TOTAL + 1))
            label="${rel}#${block_num}"

            if is_repl "$block"; then
                SKIP=$((SKIP + 1))
                echo "  SKIP  $label (REPL session)"
                continue
            fi

            if is_runnable "$block"; then
                echo "$block" > "$TMPDIR/test.nula"
                if $NULANG "$TMPDIR/test.nula" >/dev/null 2>&1; then
                    PASS=$((PASS + 1))
                    echo "  PASS  $label (run)"
                elif $NULANG --check "$TMPDIR/test.nula" >/dev/null 2>&1; then
                    PASS=$((PASS + 1))
                    echo "  PASS  $label (check only)"
                else
                    FAIL=$((FAIL + 1))
                    echo "  FAIL  $label"
                    $NULANG --check "$TMPDIR/test.nula" 2>&1 | head -3 | sed 's/^/        /'
                fi
            else
                echo "$block" > "$TMPDIR/test.nula"
                if $NULANG --check "$TMPDIR/test.nula" >/dev/null 2>&1; then
                    PASS=$((PASS + 1))
                    echo "  OK    $label (check)"
                else
                    FAIL=$((FAIL + 1))
                    echo "  FAIL  $label"
                    $NULANG --check "$TMPDIR/test.nula" 2>&1 | head -3 | sed 's/^/        /'
                fi
            fi
            continue
        fi
        if [[ $in_block -eq 1 ]]; then
            block="${block}${line}"$'\n'
        fi
    done < "$file"
done < <(find "$DOCS_DIR" -name '*.md' -o -name '*.mdx' -print0)

echo ""
echo "=== Results: $PASS passed, $FAIL failed, $SKIP skipped ($TOTAL total) ==="
[[ $FAIL -eq 0 ]] && exit 0 || exit 1
