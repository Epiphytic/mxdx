#!/usr/bin/env bash
# check-no-unencrypted-sends.sh
#
# CI grep gate that enforces the project's E2EE cardinal rule (CLAUDE.md):
# "every Matrix event and every byte on the P2P data channel must be
# end-to-end encrypted -- no exceptions."
#
# Fails (exit non-zero) if the scanned path contains any of the patterns
# 'send_raw', 'skip_encryption', or 'unencrypted' in non-test, non-doc
# Rust source. Doc comments (lines beginning with //!, ///, or //) and
# files under /tests/ or /target/ are excluded.
#
# Usage:
#   check-no-unencrypted-sends.sh                 # scan crates/mxdx-p2p/
#   check-no-unencrypted-sends.sh <path>          # scan a custom path
#   check-no-unencrypted-sends.sh --self-test     # verify the script
#                                                  itself fails on a
#                                                  synthetic violation

set -euo pipefail

PATTERN='send_raw|skip_encryption|unencrypted'
DEFAULT_SCAN_PATH="crates/mxdx-p2p"

self_test() {
    local tmpdir
    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' RETURN
    cat > "$tmpdir/violation.rs" <<'RUST'
pub fn bad() {
    matrix_client.send_raw(payload);
}
RUST
    if "$0" "$tmpdir" >/dev/null 2>&1; then
        echo "SELF-TEST FAILED: synthetic 'send_raw' violation was not detected" >&2
        return 1
    fi
    echo "self-test ok: synthetic violation correctly rejected"
}

if [[ "${1:-}" == "--self-test" ]]; then
    self_test
    exit $?
fi

SCAN_PATH="${1:-$DEFAULT_SCAN_PATH}"

if [[ ! -e "$SCAN_PATH" ]]; then
    echo "error: scan path does not exist: $SCAN_PATH" >&2
    exit 2
fi

# Strip Rust line comments before grepping. Doc comments and ordinary //
# comments are excluded so that prose explaining what NOT to do (e.g.,
# "do not call send_raw") does not trigger the gate.
matches="$(
    find "$SCAN_PATH" -type f -name '*.rs' \
        -not -path '*/target/*' \
        -not -path '*/tests/*' \
        -print0 |
    xargs -0 -r awk '
        /^[[:space:]]*\/\// { next }
        { sub(/[[:space:]]*\/\/.*$/, ""); if (length($0) > 0) print FILENAME ":" FNR ":" $0 }
    ' |
    grep -E "$PATTERN" || true
)"

if [[ -n "$matches" ]]; then
    echo "FAIL: forbidden pattern (${PATTERN}) found in $SCAN_PATH" >&2
    echo "$matches" >&2
    exit 1
fi

echo "ok: no forbidden patterns in $SCAN_PATH"
