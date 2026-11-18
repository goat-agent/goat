#!/bin/sh
set -eu

repo="goat-agent/goat"
dir="${GOAT_INSTALL_DIR:-$HOME/.local/bin}"

case "$(uname -s)" in
Darwin) os="apple-darwin" ;;
Linux) os="unknown-linux-gnu" ;;
*)
	echo "goat runs on macOS and Linux" >&2
	exit 1
	;;
esac

case "$(uname -m)" in
x86_64 | amd64) arch="x86_64" ;;
arm64 | aarch64) arch="aarch64" ;;
*)
	echo "unsupported architecture: $(uname -m)" >&2
	exit 1
	;;
esac

archive="goat-$arch-$os.tar.gz"
url="https://github.com/$repo/releases/latest/download/$archive"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL "$url" -o "$tmp/$archive"
curl -fsSL "$url.sha256" -o "$tmp/$archive.sha256"

cd "$tmp"
if command -v sha256sum >/dev/null 2>&1; then
	sha256sum -c "$archive.sha256" >/dev/null
else
	shasum -a 256 -c "$archive.sha256" >/dev/null
fi
tar -xzf "$archive"

mkdir -p "$dir"
install -m 0755 goat "$dir/goat"
echo "installed goat to $dir/goat"

case ":$PATH:" in
*":$dir:"*) ;;
*) echo "add $dir to your PATH" ;;
esac
