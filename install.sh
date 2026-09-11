#!/bin/sh
# Install commentbuster and its Claude Code hook from a GitHub release.
#
#   curl -fsSL https://raw.githubusercontent.com/NoahDavey/commentbuster/main/install.sh | sh
#
# Environment variables:
#   COMMENTBUSTER_VERSION       release tag to install, e.g. v0.1.0   (default: latest)
#   COMMENTBUSTER_INSTALL_DIR   directory for the executables          (default: ~/.local/bin)
set -eu

REPO="NoahDavey/commentbuster"
BIN="commentbuster"
VERSION="${COMMENTBUSTER_VERSION:-latest}"
INSTALL_DIR="${COMMENTBUSTER_INSTALL_DIR:-$HOME/.local/bin}"

fail() { echo "install.sh: $*" >&2; exit 1; }

command -v curl >/dev/null 2>&1 || fail "curl is required"

case "$(uname -s)" in
  Darwin) os="apple-darwin" ;;
  Linux)  os="unknown-linux-gnu" ;;
  *)      fail "unsupported OS $(uname -s); build from source with cargo instead" ;;
esac
case "$(uname -m)" in
  arm64|aarch64) arch="aarch64" ;;
  x86_64|amd64)  arch="x86_64" ;;
  *)             fail "unsupported architecture $(uname -m); build from source with cargo instead" ;;
esac
target="$arch-$os"
asset="$BIN-$target.tar.gz"

if [ "$VERSION" = "latest" ]; then
  base="https://github.com/$REPO/releases/latest/download"
else
  base="https://github.com/$REPO/releases/download/$VERSION"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "Downloading $asset ($VERSION)"
curl -fsSL "$base/$asset" -o "$tmp/$asset" || fail "download failed: $base/$asset"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" || fail "download failed: $base/SHA256SUMS"

expected=$(grep " $asset\$" "$tmp/SHA256SUMS" | cut -d' ' -f1)
[ -n "$expected" ] || fail "no entry for $asset in SHA256SUMS"
if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$tmp/$asset" | cut -d' ' -f1)
elif command -v shasum >/dev/null 2>&1; then
  actual=$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)
else
  fail "neither sha256sum nor shasum is available to verify the download"
fi
[ "$actual" = "$expected" ] || fail "checksum mismatch for $asset"

tar xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$INSTALL_DIR"
install -m 755 "$tmp/$BIN-$target/$BIN" "$INSTALL_DIR/$BIN"
install -m 755 "$tmp/$BIN-$target/$BIN-claude-gate" "$INSTALL_DIR/$BIN-claude-gate"

echo "Installed $("$INSTALL_DIR/$BIN" --version) to $INSTALL_DIR"
echo "Claude Code hook: $INSTALL_DIR/$BIN-claude-gate"

case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *)
    echo
    echo "$INSTALL_DIR is not on your PATH. To call commentbuster directly, add it:"
    echo "  echo 'export PATH=\"$INSTALL_DIR:\$PATH\"' >> ~/.zshrc"
    ;;
esac
