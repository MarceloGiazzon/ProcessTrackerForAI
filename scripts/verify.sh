#!/bin/sh
# Runs every check a change must pass, in the same order as CI, and stops at
# the first failure. Usage (from the repository root):
#
#   ./scripts/verify.sh
#
# It checks formatting, lints for Windows, Linux and macOS (cross-checking
# needs no machine of that kind), and runs all tests on this machine.
set -eu
cd "$(dirname "$0")/.."

step() {
    name="$1"
    shift
    printf '==> %s\n' "$name"
    if ! "$@"; then
        printf 'FAILED: %s\n' "$name" >&2
        exit 1
    fi
}

case "$(uname -s)" in
    Darwin) targets="x86_64-unknown-linux-musl x86_64-pc-windows-msvc" ;;
    Linux) targets="aarch64-apple-darwin x86_64-pc-windows-msvc" ;;
    *) targets="x86_64-unknown-linux-musl aarch64-apple-darwin" ;; # Git Bash on Windows
esac

installed="$(rustup target list --installed)"
for t in $targets; do
    case "$installed" in
        *"$t"*) ;;
        *) step "add Rust target $t (one-time download)" rustup target add "$t" ;;
    esac
done

step "format check (fix with: cargo fmt)" cargo fmt --check
step "lint (this machine)" cargo clippy --all-targets -- -D warnings
for t in $targets; do
    step "lint ($t)" cargo clippy --all-targets --target "$t" -- -D warnings
done
step "tests" cargo test
printf 'ALL CHECKS PASSED\n'
