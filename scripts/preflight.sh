#!/usr/bin/env bash
set -euo pipefail

PASS=0
FAIL=0
BLOCKED_PHASES=""

check() {
    local name="$1" cmd="$2" blocks="$3" install="$4"
    if command -v "$cmd" &>/dev/null || eval "$cmd" &>/dev/null 2>&1; then
        echo "  [PASS] $name ($(command -v "$cmd" 2>/dev/null || echo 'found'))"
        ((++PASS))
    else
        echo "  [FAIL] $name -- not found"
        echo "         Install: $install"
        echo "         Blocks: $blocks"
        ((++FAIL))
        BLOCKED_PHASES="$BLOCKED_PHASES $blocks"
    fi
}

check_lib() {
    local name="$1" pkg="$2" blocks="$3" install="$4"
    if pkg-config --exists "$pkg" 2>/dev/null; then
        echo "  [PASS] $name"
        ((++PASS))
    else
        echo "  [FAIL] $name -- not found"
        echo "         Install: $install"
        echo "         Blocks: $blocks"
        ((++FAIL))
        BLOCKED_PHASES="$BLOCKED_PHASES $blocks"
    fi
}

echo "mxdx Preflight Check"
echo "===================="
echo ""

check "cargo" "cargo" "Phase 1+" "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
check "rustc" "rustc" "Phase 1+" "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
check "node" "node" "Phase 2+ (TS types)" "install via nvm or package manager"
check "npm" "npm" "Phase 2+ (TS types)" "comes with node"
check "git" "git" "Phase 1+" "sudo apt-get install -y git"
check "tuwunel" "tuwunel" "Phase 3+" "see docs/adr/2026-03-05-tuwunel-ground-truth.md"
check "tmux" "tmux" "Phase 6 (interactive terminals)" "sudo apt-get install -y tmux"

check_lib "libsqlite3-dev" "sqlite3" "Phase 4+ (matrix-sdk)" "sudo apt-get install -y libsqlite3-dev"
check_lib "libssl-dev" "openssl" "Phase 4+ (matrix-sdk)" "sudo apt-get install -y libssl-dev"

check "softhsm2-util" "softhsm2-util" "Phase 9 (secrets)" "sudo apt-get install -y softhsm2"
check "mkcert" "mkcert" "Phase 11 (federation TLS)" "go install filippo.io/mkcert@latest"

if rustup target list --installed 2>/dev/null | grep -q "wasm32-wasip2"; then
    echo "  [PASS] wasm32-wasip2 target"
    ((++PASS))
else
    echo "  [FAIL] wasm32-wasip2 target -- not installed"
    echo "         Install: rustup target add wasm32-wasip2"
    echo "         Blocks: Phase 12 (WASI)"
    ((++FAIL))
    BLOCKED_PHASES="$BLOCKED_PHASES Phase-12"
fi

if npx playwright --version &>/dev/null 2>&1; then
    echo "  [PASS] playwright"
    ((++PASS))
else
    echo "  [FAIL] playwright -- not found"
    echo "         Install: npx playwright install"
    echo "         Blocks: Phase 7 (browser E2E)"
    ((++FAIL))
    BLOCKED_PHASES="$BLOCKED_PHASES Phase-7"
fi

echo ""
echo "Results: $PASS passed, $FAIL failed"

if [ "$FAIL" -gt 0 ]; then
    echo "Blocked phases:$BLOCKED_PHASES"
    echo ""
    read -p "Proceed with gaps? [y/N] " -n 1 -r
    echo
    if [[ ! $REPLY =~ ^[Yy]$ ]]; then
        echo "Install missing tools and re-run."
        exit 1
    fi
    echo "Proceeding with acknowledged gaps."
fi
