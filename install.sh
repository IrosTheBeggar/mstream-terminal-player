#!/bin/sh
# Installs mstream-player on Linux and macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/IrosTheBeggar/mstream-terminal-player/main/install.sh | sh
#
# Fetches the binary for this machine from the latest GitHub release,
# checks its sha256 against the release's manifest.json, and installs it
# as `mstream-player`. Configuration, all optional:
#
#   MSTREAM_PLAYER_VERSION      a tag like v0.1.0 (default: latest)
#   MSTREAM_PLAYER_INSTALL_DIR  where the binary goes (default: ~/.local/bin)
#
# POSIX sh on purpose: NAS boxes and minimal images do not all carry bash.
set -eu

REPO="IrosTheBeggar/mstream-terminal-player"
DIR="${MSTREAM_PLAYER_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${MSTREAM_PLAYER_VERSION:-latest}"

os=$(uname -s)
arch=$(uname -m)
case "$os" in
    Darwin) platform=darwin ;;
    Linux) platform=linux ;;
    *)
        echo "unsupported OS: $os — on Windows, use install.ps1" >&2
        exit 1
        ;;
esac
case "$platform-$arch" in
    darwin-x86_64) asset="mstream-player-darwin-x64" ;;
    darwin-arm64) asset="mstream-player-darwin-arm64" ;;
    linux-x86_64 | linux-amd64) asset="mstream-player-linux-x64" ;;
    linux-aarch64 | linux-arm64) asset="mstream-player-linux-arm64" ;;
    linux-armv7l) asset="mstream-player-linux-arm" ;;
    linux-armv6l)
        # A Pi Zero or Pi 1: the armv7 build will not run there, and a
        # download that dies with "Illegal instruction" is worse than a no.
        echo "armv6 has no prebuilt binary — build from source with cargo" >&2
        exit 1
        ;;
    *)
        echo "no prebuilt binary for $os/$arch — build from source with cargo" >&2
        exit 1
        ;;
esac

if [ "$VERSION" = "latest" ]; then
    base="https://github.com/$REPO/releases/latest/download"
else
    base="https://github.com/$REPO/releases/download/$VERSION"
fi

fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$2" "$1"
    else
        echo "this installer needs curl or wget" >&2
        exit 1
    fi
}

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

echo "fetching $asset ($VERSION)..."
fetch "$base/$asset" "$tmp/$asset"
fetch "$base/manifest.json" "$tmp/manifest.json"

# The hash for this asset, out of the manifest published beside it. No jq
# on a fresh box, so the line is picked apart with the tools sh always has.
expected=$(grep -o "\"file\": \"$asset\", \"sha256\": \"[0-9a-f]*\"" "$tmp/manifest.json" \
    | grep -o '[0-9a-f]\{64\}' || true)
if [ -z "$expected" ]; then
    echo "manifest.json has no entry for $asset — refusing to install" >&2
    exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$tmp/$asset" | cut -d' ' -f1)
else
    actual=$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)
fi
if [ "$actual" != "$expected" ]; then
    echo "sha256 mismatch for $asset — download corrupted, not installing" >&2
    echo "  expected $expected" >&2
    echo "  got      $actual" >&2
    exit 1
fi

mkdir -p "$DIR"
cp "$tmp/$asset" "$DIR/mstream-player"
chmod 755 "$DIR/mstream-player"
echo "installed $("$DIR/mstream-player" --version 2>/dev/null || echo "mstream-player") to $DIR"

# Two notes, each only when it applies: a PATH that will not find the
# binary, and the one shared library the Linux builds expect.
case ":$PATH:" in
    *":$DIR:"*) ;;
    *) echo "note: $DIR is not on your PATH" ;;
esac
if ! "$DIR/mstream-player" --version >/dev/null 2>&1; then
    echo "note: the binary did not run — on Linux it needs ALSA installed" >&2
    echo "      (debian/ubuntu: sudo apt install libasound2)" >&2
fi
