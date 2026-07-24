set -eu

repo=goat-agent/goat
base=https://github.com/$repo/releases/latest/download
app_home="${HOME}/.goat"
bin_dir="${app_home}/bin"
env_file="${app_home}/env"
source_line='. "$HOME/.goat/env"'
purple=''
reset=''
if [ -t 1 ]; then
    purple='\033[35m'
    reset='\033[0m'
fi
printf "%bgoat%b installer\n" "$purple" "$reset"

need() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "missing required command: $1" >&2
        exit 1
    fi
}

need uname
need tar
need mktemp
need grep
need awk

if command -v curl >/dev/null 2>&1; then
    fetch() {
        curl -fsSL "$1" -o "$2"
    }
elif command -v wget >/dev/null 2>&1; then
    fetch() {
        wget -qO "$2" "$1"
    }
else
    echo "missing required command: curl or wget" >&2
    exit 1
fi

os=$(uname -s)
arch=$(uname -m)
case "$os:$arch" in
    Linux:x86_64)
        target=x86_64-unknown-linux-gnu
        ;;
    Linux:aarch64|Linux:arm64)
        target=aarch64-unknown-linux-gnu
        ;;
    Darwin:x86_64)
        target=x86_64-apple-darwin
        ;;
    Darwin:arm64)
        target=aarch64-apple-darwin
        ;;
    *)
        echo "unsupported platform: $os $arch" >&2
        exit 1
        ;;
esac

if command -v sha256sum >/dev/null 2>&1; then
    verify() {
        expected=$(grep "  $archive" "$tmp/SHA256SUMS" | awk '{print $1}')
        actual=$(sha256sum "$tmp/$archive" | awk '{print $1}')
        test -n "$expected" && test "$expected" = "$actual"
    }
elif command -v shasum >/dev/null 2>&1; then
    verify() {
        expected=$(grep "  $archive" "$tmp/SHA256SUMS" | awk '{print $1}')
        actual=$(shasum -a 256 "$tmp/$archive" | awk '{print $1}')
        test -n "$expected" && test "$expected" = "$actual"
    }
else
    echo "missing required command: sha256sum or shasum" >&2
    exit 1
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
archive=goat-$target.tar.gz
fetch "$base/$archive" "$tmp/$archive"
fetch "$base/SHA256SUMS" "$tmp/SHA256SUMS"
if ! verify; then
    echo "checksum verification failed for $archive" >&2
    exit 1
fi
mkdir -p "$tmp/extract"
tar -xzf "$tmp/$archive" -C "$tmp/extract"
test -f "$tmp/extract/goat"

mkdir -p "$bin_dir"
cp "$tmp/extract/goat" "$bin_dir/goat"
chmod 755 "$bin_dir/goat"

cat > "$env_file" <<'ENV'
case ":${PATH}:" in
    *:"$HOME/.goat/bin":*)
        ;;
    *)
        export PATH="$HOME/.goat/bin:$PATH"
        ;;
esac
ENV

add_source() {
    dir=$(dirname "$1")
    [ -d "$dir" ] || return 0
    if [ -f "$1" ] && grep -qF "$source_line" "$1" 2>/dev/null; then
        return 0
    fi
    printf '\n%s\n' "$source_line" >> "$1"
}

add_source_if_exists() {
    [ -f "$1" ] || return 0
    add_source "$1"
}

add_source "$HOME/.profile"
add_source "$HOME/.bashrc"
add_source "${ZDOTDIR:-$HOME}/.zshenv"
add_source_if_exists "$HOME/.bash_profile"
add_source_if_exists "$HOME/.bash_login"

fish_root="${XDG_CONFIG_HOME:-$HOME/.config}/fish"
if [ -d "$fish_root" ] || command -v fish >/dev/null 2>&1; then
    mkdir -p "$fish_root/conf.d"
    cat > "$fish_root/conf.d/goat.fish" <<'FISH'
if not contains "$HOME/.goat/bin" $PATH
    set -gx PATH "$HOME/.goat/bin" $PATH
end
FISH
fi

printf "%bgoat%b installed to %s\n" "$purple" "$reset" "$bin_dir/goat"

case ":${PATH}:" in
    *:"$bin_dir":*)
        ;;
    *)
        echo "Run this now, or open a new terminal, to start using goat:"
        echo "  $source_line"
        ;;
esac

if [ -e /usr/local/bin/goat ] || [ -e /usr/local/bin/goat-update ]; then
    echo "An older system install was found. Remove it so it no longer shadows the new one:"
    echo "  sudo rm -f /usr/local/bin/goat /usr/local/bin/goat-update"
    echo "(the current installer uses $bin_dir and never writes to system directories)"
fi
