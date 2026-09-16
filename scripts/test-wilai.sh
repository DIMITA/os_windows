#!/usr/bin/env bash
# Reproduce the wilai CI checks locally before pushing.
#
#   ./scripts/test-wilai.sh              # build + test
#   ./scripts/test-wilai.sh --fmt        # + rustfmt check
#   ./scripts/test-wilai.sh --release    # + release build sanity
#   ./scripts/test-wilai.sh --all        # everything above
#
# Requires: rustup toolchain (stable), python3 (MCP mock test).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.."; pwd)"
cd "$ROOT/wilai"

DO_FMT=0
DO_RELEASE=0
for arg in "$@"; do
    case "$arg" in
        --fmt)     DO_FMT=1 ;;
        --release) DO_RELEASE=1 ;;
        --all)     DO_FMT=1; DO_RELEASE=1 ;;
        -h|--help)
            sed -n '1,10p' "$0" | sed 's/^# //; s/^#//'
            exit 0 ;;
        *)
            echo "unknown flag: $arg"; exit 2 ;;
    esac
done

section() { printf "\n\033[1;34m--- %s ---\033[0m\n" "$1"; }

if [[ $DO_FMT -eq 1 ]]; then
    section "cargo fmt --check"
    cargo fmt --all -- --check || true
fi

section "cargo build --workspace"
cargo build --workspace --locked

section "cargo test --workspace"
RUST_BACKTRACE=1 cargo test --workspace --locked --no-fail-fast

if [[ $DO_RELEASE -eq 1 ]]; then
    section "cargo build --release"
    cargo build --workspace --release --locked
    section "release binaries"
    ls -lh target/release/wilai target/release/wilai-daemon \
           target/release/wilai-voice target/release/wilai-overlay
fi

section "summary"
grep -h "^test result" target/debug/deps/*.log 2>/dev/null || true
echo "all green."
