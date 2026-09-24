#!/bin/sh
# Install the latest git-wrapped release binary.
#   curl -fsSL https://github.com/adawg-dev/git-wrapped/releases/latest/download/install.sh | sh
# Env: GIT_WRAPPED_VERSION (e.g. v0.1.0, default latest), INSTALL_DIR (default ~/.local/bin)
set -eu

repo=adawg-dev/git-wrapped
dir=${INSTALL_DIR:-$HOME/.local/bin}

case "$(uname -s)" in
  Darwin) os=apple-darwin ;;
  Linux) os=unknown-linux-musl ;;
  *) echo "Unsupported OS: $(uname -s). Download a build from https://github.com/$repo/releases" >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64 | amd64) arch=x86_64 ;;
  arm64 | aarch64) arch=aarch64 ;;
  *) echo "Unsupported CPU: $(uname -m)" >&2; exit 1 ;;
esac

asset=git-wrapped-$arch-$os.tar.gz
if [ -n "${GIT_WRAPPED_VERSION:-}" ]; then
  base=https://github.com/$repo/releases/download/$GIT_WRAPPED_VERSION
else
  base=https://github.com/$repo/releases/latest/download
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
echo "Downloading $asset..."
curl -fsSL "$base/$asset" -o "$tmp/$asset"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS"

expected=$(grep " $asset\$" "$tmp/SHA256SUMS" | cut -d' ' -f1)
if command -v sha256sum >/dev/null; then
  actual=$(sha256sum "$tmp/$asset" | cut -d' ' -f1)
else
  actual=$(shasum -a 256 "$tmp/$asset" | cut -d' ' -f1)
fi
if [ -z "$expected" ] || [ "$expected" != "$actual" ]; then
  echo "Checksum mismatch for $asset" >&2
  exit 1
fi

tar -xzf "$tmp/$asset" -C "$tmp"
mkdir -p "$dir"
mv "$tmp/git-wrapped" "$dir/git-wrapped"
chmod +x "$dir/git-wrapped"
echo "Installed git-wrapped to $dir/git-wrapped"

case ":$PATH:" in
  *":$dir:"*) ;;
  *) echo "Add $dir to your PATH, e.g.: echo 'export PATH=\"$dir:\$PATH\"' >> ~/.zshrc" ;;
esac
