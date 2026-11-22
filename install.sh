#!/bin/sh
# tvpn installer for Linux and macOS (also WSL).
#
#   curl -fsSL https://raw.githubusercontent.com/AbhijeetKumar1505/T-VPN/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/AbhijeetKumar1505/T-VPN/main/install.sh | sh -s -- --version 0.1.0 --dir ~/.local/bin
#
# Environment overrides: TVPN_VERSION, TVPN_INSTALL_DIR, TVPN_REPO,
# TVPN_DOWNLOAD_BASE (a mirror URL containing the release assets).
# The download is verified against the release's SHA256SUMS before installing.

set -eu

REPO="${TVPN_REPO:-AbhijeetKumar1505/T-VPN}"
VERSION="${TVPN_VERSION:-latest}"
INSTALL_DIR="${TVPN_INSTALL_DIR:-}"

say() { printf '%s\n' "tvpn-install: $*"; }
err() { printf '%s\n' "tvpn-install: error: $*" >&2; exit 1; }
has() { command -v "$1" >/dev/null 2>&1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --version) VERSION="${2:?--version needs a value}"; shift 2 ;;
        --version=*) VERSION="${1#*=}"; shift ;;
        --dir) INSTALL_DIR="${2:?--dir needs a value}"; shift 2 ;;
        --dir=*) INSTALL_DIR="${1#*=}"; shift ;;
        --repo) REPO="${2:?--repo needs a value}"; shift 2 ;;
        -h|--help)
            sed -n '2,10p' "$0" 2>/dev/null || true
            exit 0 ;;
        *) err "unknown option: $1" ;;
    esac
done

os="$(uname -s)"
arch="$(uname -m)"
case "$arch" in
    x86_64|amd64) arch="x86_64" ;;
    aarch64|arm64) arch="aarch64" ;;
    *) err "unsupported CPU architecture: $arch (build from source: cargo install tvpn)" ;;
esac
case "$os" in
    Linux) target="${arch}-unknown-linux-musl" ;;
    Darwin) target="${arch}-apple-darwin" ;;
    MINGW*|MSYS*|CYGWIN*) err "on Windows use: winget install tvpn, or the PowerShell installer (install.ps1)" ;;
    *) err "unsupported OS: $os" ;;
esac

VERSION="${VERSION#v}"
if [ -n "${TVPN_DOWNLOAD_BASE:-}" ]; then
    base="${TVPN_DOWNLOAD_BASE%/}"  # mirror / air-gapped installs
elif [ "$VERSION" = "latest" ]; then
    base="https://github.com/${REPO}/releases/latest/download"
else
    base="https://github.com/${REPO}/releases/download/v${VERSION}"
fi
asset="tvpn-${target}.tar.gz"

download() { # url dest
    if has curl; then
        if [ -n "${TVPN_DOWNLOAD_BASE:-}" ]; then
            curl -fsSL "$1" -o "$2"
        else
            curl --proto '=https' --tlsv1.2 -fsSL "$1" -o "$2"
        fi
    elif has wget; then
        if [ -n "${TVPN_DOWNLOAD_BASE:-}" ]; then
            wget -q "$1" -O "$2"
        else
            wget -q --https-only "$1" -O "$2"
        fi
    else
        err "need curl or wget"
    fi
}

sha256() {
    if has sha256sum; then sha256sum "$1" | awk '{print $1}'
    elif has shasum; then shasum -a 256 "$1" | awk '{print $1}'
    elif has openssl; then openssl dgst -sha256 "$1" | awk '{print $NF}'
    else err "need sha256sum, shasum or openssl to verify the download"
    fi
}

tmp="$(mktemp -d 2>/dev/null || mktemp -d -t tvpn)"
trap 'rm -rf "$tmp"' EXIT INT TERM

say "downloading ${asset} (${VERSION})"
download "${base}/${asset}" "${tmp}/${asset}" || err "download failed: ${base}/${asset}"
download "${base}/SHA256SUMS" "${tmp}/SHA256SUMS" || err "could not download SHA256SUMS"

expected="$(grep " \*\{0,1\}${asset}\$" "${tmp}/SHA256SUMS" | awk '{print $1}' | head -n1)"
[ -n "$expected" ] || err "${asset} is not listed in SHA256SUMS"
actual="$(sha256 "${tmp}/${asset}")"
[ "$expected" = "$actual" ] || err "checksum mismatch for ${asset} (expected ${expected}, got ${actual})"
say "checksum verified"

tar -xzf "${tmp}/${asset}" -C "$tmp"
bin="$(find "$tmp" -type f -name tvpn | head -n1)"
[ -n "$bin" ] || err "archive did not contain the tvpn binary"
chmod +x "$bin"

if [ -z "$INSTALL_DIR" ]; then
    if [ -w /usr/local/bin ]; then
        INSTALL_DIR=/usr/local/bin
    elif has sudo && [ -d /usr/local/bin ]; then
        INSTALL_DIR=/usr/local/bin
    else
        INSTALL_DIR="${HOME}/.local/bin"
    fi
fi

if mkdir -p "$INSTALL_DIR" 2>/dev/null && [ -w "$INSTALL_DIR" ]; then
    install -m 0755 "$bin" "${INSTALL_DIR}/tvpn" 2>/dev/null || cp "$bin" "${INSTALL_DIR}/tvpn"
else
    say "installing to ${INSTALL_DIR} requires sudo"
    sudo mkdir -p "$INSTALL_DIR"
    sudo install -m 0755 "$bin" "${INSTALL_DIR}/tvpn"
fi

say "installed $("${INSTALL_DIR}/tvpn" --version) to ${INSTALL_DIR}/tvpn"
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *) say "note: ${INSTALL_DIR} is not on your PATH; add it to your shell profile" ;;
esac
say "next: run 'tvpn doctor', then 'tvpn import <file.conf>' and 'tvpn up'"
