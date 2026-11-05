#!/bin/sh
set -eu

repo="Avead556/ttywright"
version="${TTYWRIGHT_VERSION:-latest}"
home_dir="${TTYWRIGHT_HOME:-$HOME/.local/share/ttywright}"
bin_dir="${TTYWRIGHT_BIN_DIR:-$HOME/.local/bin}"

say() {
    printf '%s\n' "$*"
}

fail() {
    printf 'ttywright: %s\n' "$*" >&2
    exit 1
}

fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl --fail --location --silent --show-error --output "$2" "$1"
    elif command -v wget >/dev/null 2>&1; then
        wget --quiet --output-document="$2" "$1"
    else
        fail "need curl or wget"
    fi
}

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | cut -d ' ' -f 1
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | cut -d ' ' -f 1
    else
        fail "need sha256sum or shasum to check the download"
    fi
}

case "$(uname -s)" in
    Darwin) os=apple-darwin ;;
    Linux) os=unknown-linux-gnu ;;
    *) fail "no build for $(uname -s); build from source: cargo install --path ." ;;
esac

case "$(uname -m)" in
    arm64 | aarch64) arch=aarch64 ;;
    x86_64 | amd64) arch=x86_64 ;;
    *) fail "no build for $(uname -m); build from source: cargo install --path ." ;;
esac

if [ "$os" = apple-darwin ] && [ "$arch" = x86_64 ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = 1 ]; then
    arch=aarch64
fi

triple="$arch-$os"
name="ttywright-$triple"

if [ "$version" = latest ]; then
    base="https://github.com/$repo/releases/latest/download"
else
    base="https://github.com/$repo/releases/download/$version"
fi

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM

if [ -n "${TTYWRIGHT_ARCHIVE:-}" ]; then
    cp "$TTYWRIGHT_ARCHIVE" "$work/$name.tar.gz"
    cp "$TTYWRIGHT_ARCHIVE.sha256" "$work/$name.tar.gz.sha256"
else
    say "Downloading $name ($version)"
    fetch "$base/$name.tar.gz" "$work/$name.tar.gz" ||
        fail "could not download $base/$name.tar.gz"
    fetch "$base/$name.tar.gz.sha256" "$work/$name.tar.gz.sha256" ||
        fail "could not download the checksum"
fi

expected=$(cut -d ' ' -f 1 "$work/$name.tar.gz.sha256")
actual=$(sha256 "$work/$name.tar.gz")
[ "$expected" = "$actual" ] || fail "checksum mismatch: expected $expected, got $actual"

tar -xzf "$work/$name.tar.gz" -C "$work"
[ -x "$work/$name/ttyw" ] || fail "the archive has no ttyw"

if [ -e "$home_dir" ]; then
    [ -f "$home_dir/ttyw" ] || fail "$home_dir exists and is not a ttywright install; set TTYWRIGHT_HOME"
    rm -rf "$home_dir"
fi
mkdir -p "$(dirname "$home_dir")" "$bin_dir"
mv "$work/$name" "$home_dir"
ln -sf "$home_dir/ttyw" "$bin_dir/ttyw"

say "Installed ttyw to $home_dir"
say "Linked $bin_dir/ttyw"

case ":$PATH:" in
    *":$bin_dir:"*) say "Run: ttyw" ;;
    *)
        say ""
        say "$bin_dir is not on your PATH. Add it:"
        case "${SHELL:-}" in
            */fish) say "  fish_add_path $bin_dir" ;;
            */bash) say "  echo 'export PATH=\"$bin_dir:\$PATH\"' >> ~/.bashrc" ;;
            *) say "  echo 'export PATH=\"$bin_dir:\$PATH\"' >> ~/.zshrc" ;;
        esac
        ;;
esac
