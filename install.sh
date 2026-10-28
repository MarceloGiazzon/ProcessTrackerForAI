#!/bin/sh
# Installs the latest ptrack release on Linux or macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/MarceloGiazzon/ProcessTrackerForAI/main/install.sh | sh
#
# Options (environment variables):
#   PTRACK_VERSION=v0.1.0       install a specific version instead of the latest
#   PTRACK_INSTALL_DIR=/path    where to put the binary (default: ~/.local/bin)
set -eu

REPO="MarceloGiazzon/ProcessTrackerForAI"
DIR="${PTRACK_INSTALL_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
fail() { printf 'ptrack install: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "this script needs '$1'"; }

need uname
need tar
if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -qO "$2" "$1"; }
else
    fail "this script needs 'curl' or 'wget'"
fi

case "$(uname -s)" in
    Linux) os="unknown-linux-musl" ;;
    Darwin) os="apple-darwin" ;;
    *) fail "unsupported system $(uname -s); on Windows use install.ps1" ;;
esac
case "$(uname -m)" in
    x86_64 | amd64) arch="x86_64" ;;
    aarch64 | arm64) arch="aarch64" ;;
    *) fail "unsupported processor $(uname -m)" ;;
esac
target="$arch-$os"

if [ -n "${PTRACK_VERSION:-}" ]; then
    version="$PTRACK_VERSION"
else
    tmp_json="$(mktemp)"
    fetch "https://api.github.com/repos/$REPO/releases/latest" "$tmp_json" ||
        fail "could not reach GitHub to find the latest version"
    version="$(sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' "$tmp_json" | head -n 1)"
    rm -f "$tmp_json"
    [ -n "$version" ] || fail "no published release found"
fi

name="ptrack-$version-$target.tar.gz"
url="https://github.com/$REPO/releases/download/$version/$name"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

say "Downloading ptrack $version for $target…"
fetch "$url" "$work/$name" || fail "download failed: $url"
if fetch "$url.sha256" "$work/$name.sha256" 2>/dev/null; then
    expected="$(cut -d ' ' -f 1 "$work/$name.sha256")"
    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$work/$name" | cut -d ' ' -f 1)"
    else
        actual="$(shasum -a 256 "$work/$name" | cut -d ' ' -f 1)"
    fi
    [ "$expected" = "$actual" ] || fail "checksum mismatch; the download may be corrupted"
fi

tar xzf "$work/$name" -C "$work"
mkdir -p "$DIR"
install -m 755 "$work/ptrack" "$DIR/ptrack" 2>/dev/null || { cp "$work/ptrack" "$DIR/ptrack" && chmod 755 "$DIR/ptrack"; }

say "Installed: $DIR/ptrack"
case ":$PATH:" in
    *":$DIR:"*) "$DIR/ptrack" version ;;
    *)
        say ""
        say "$DIR is not on your PATH yet. Add it with:"
        say "  echo 'export PATH=\"$DIR:\$PATH\"' >> ~/.bashrc && . ~/.bashrc"
        ;;
esac
