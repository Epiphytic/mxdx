#!/usr/bin/env bash
# check-perf-streak.sh
#
# Reads the last N nightly perf result files from the results directory and
# reports whether they constitute a green streak. Used as a gate for the
# Phase-9 default-on flip (T-90): `p2p_enabled` cannot be set to `true`
# until 3 consecutive green runs.
#
# Usage:
#   check-perf-streak.sh [N]           # check last N runs (default: 3)
#   check-perf-streak.sh --self-test   # verify the script itself works
#
# Result files are expected at:
#   packages/e2e-tests/results/nightly-perf-<ISO-DATE>.json
#
# Each result file must contain a top-level "status" field:
#   { "status": "green", ... }  or  { "status": "red", ... }
#
# Exit codes:
#   0  — streak of N consecutive green runs confirmed
#   1  — streak broken (fewer than N results, or any non-green)
#   2  — usage error or missing directory

set -euo pipefail

RESULTS_DIR="${RESULTS_DIR:-packages/e2e-tests/results}"
NIGHTLY_PREFIX="nightly-perf-"

self_test() {
    local tmpdir
    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' RETURN

    # Case 1: 3 green runs → exit 0
    for i in 1 2 3; do
        echo '{"status":"green","date":"2026-04-1'$i'"}' > "$tmpdir/${NIGHTLY_PREFIX}2026-04-1${i}.json"
    done
    if RESULTS_DIR="$tmpdir" "$0" 3; then
        echo "  self-test case 1 (3 green): PASS"
    else
        echo "  self-test case 1 (3 green): FAIL — expected exit 0" >&2
        return 1
    fi

    # Case 2: 2 green + 1 red → exit 1
    echo '{"status":"red","date":"2026-04-14"}' > "$tmpdir/${NIGHTLY_PREFIX}2026-04-14.json"
    if ! RESULTS_DIR="$tmpdir" "$0" 3 >/dev/null 2>&1; then
        echo "  self-test case 2 (1 red in last 3): PASS"
    else
        echo "  self-test case 2 (1 red in last 3): FAIL — expected exit 1" >&2
        return 1
    fi

    # Case 3: fewer than N results → exit 1
    tmpdir2="$(mktemp -d)"
    trap 'rm -rf "$tmpdir2"' RETURN
    echo '{"status":"green"}' > "$tmpdir2/${NIGHTLY_PREFIX}2026-04-15.json"
    if ! RESULTS_DIR="$tmpdir2" "$0" 3 >/dev/null 2>&1; then
        echo "  self-test case 3 (fewer than N): PASS"
    else
        echo "  self-test case 3 (fewer than N): FAIL — expected exit 1" >&2
        return 1
    fi

    echo "self-test ok: all cases passed"
}

if [[ "${1:-}" == "--self-test" ]]; then
    self_test
    exit $?
fi

REQUIRED_STREAK="${1:-3}"

if ! [[ "$REQUIRED_STREAK" =~ ^[0-9]+$ ]] || [[ "$REQUIRED_STREAK" -lt 1 ]]; then
    echo "error: streak count must be a positive integer, got: $REQUIRED_STREAK" >&2
    exit 2
fi

if [[ ! -d "$RESULTS_DIR" ]]; then
    echo "error: results directory does not exist: $RESULTS_DIR" >&2
    exit 2
fi

# Collect nightly result files sorted by name (ISO date sorts correctly).
mapfile -t result_files < <(
    find "$RESULTS_DIR" -maxdepth 1 -name "${NIGHTLY_PREFIX}*.json" -type f | sort
)

total="${#result_files[@]}"

if [[ "$total" -lt "$REQUIRED_STREAK" ]]; then
    echo "FAIL: only $total nightly result(s) found, need $REQUIRED_STREAK consecutive green" >&2
    exit 1
fi

# Check the last N files.
start_idx=$((total - REQUIRED_STREAK))
green_count=0
for ((i = start_idx; i < total; i++)); do
    file="${result_files[$i]}"
    filename="$(basename "$file")"

    # Extract status field. Supports both jq and a grep fallback.
    if command -v jq >/dev/null 2>&1; then
        status="$(jq -r '.status // "unknown"' "$file")"
    else
        # Fallback: crude JSON parse for simple {"status":"green"} shape
        status="$(grep -oP '"status"\s*:\s*"\K[^"]+' "$file" || echo "unknown")"
    fi

    if [[ "$status" != "green" ]]; then
        echo "FAIL: $filename has status '$status' (expected 'green')" >&2
        exit 1
    fi
    green_count=$((green_count + 1))
done

echo "ok: $green_count consecutive green nightly perf runs confirmed (required: $REQUIRED_STREAK)"
exit 0
