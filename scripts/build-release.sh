#!/bin/sh
# Builds a release folder for this machine (ADR-0035): a small program, and what
# it loads when needed in lib/. Result: dist/ttywright-<triple>/.
set -eu

cd "$(dirname "$0")/.."
RUST_ANALYZER_VERSION=$(cat scripts/rust-analyzer-version)
host=$(rustc -vV | sed -n 's/^host: //p')
triple=${TARGET:-$host}
work=target/bundle/$RUST_ANALYZER_VERSION
base=https://github.com/rust-lang/rust-analyzer
server=$work/rust-analyzer-$triple
mkdir -p "$work"

if [ ! -f "$server" ]; then
    curl --fail --location --silent --show-error \
        "$base/releases/download/$RUST_ANALYZER_VERSION/rust-analyzer-$triple.gz" \
        --output "$server.gz"
    gunzip --keep --force "$server.gz"
    chmod +x "$server"
fi

profile=release-dist
if [ "$triple" = "$host" ]; then
    out=target/$profile
    set --
else
    out=target/$triple/$profile
    set -- --target "$triple"
fi
cargo build --profile "$profile" --locked "$@" --no-default-features --bin ttyw
cargo build --profile "$profile" --locked "$@" -p ttywright-plugin-host --bin ttywright-plugins

dist=dist/ttywright-$triple
rm -rf "$dist"
mkdir -p "$dist/lib/grammars"
cp "$out/ttyw" "$dist/ttyw"
cp "$out/ttywright-plugins" "$dist/lib/"
cp "$server" "$dist/lib/rust-analyzer"

case $(uname -s) in
    Darwin) extension=dylib ;;
    *) extension=so ;;
esac
# Each is the same library crate with another grammar in it, under one name.
for grammar in rust toml markdown json php javascript typescript tsx java go python bash; do
    cargo build --profile "$profile" --locked "$@" -p ttywright-grammar --features "$grammar"
    module=$dist/lib/grammars/$grammar.$extension
    cp "$out/libttywright_grammar.$extension" "$module"
    # Its own name inside would be the path it was built at, on this machine.
    if [ "$extension" = dylib ]; then
        install_name_tool -id "$grammar.$extension" "$module" 2>/dev/null
        codesign --force --sign - "$module" 2>/dev/null
    fi
done

for license in LICENSE-MIT LICENSE-APACHE; do
    curl --fail --location --silent --show-error \
        "https://raw.githubusercontent.com/rust-lang/rust-analyzer/$RUST_ANALYZER_VERSION/$license" \
        --output "$dist/rust-analyzer-$license"
done
echo "$dist"
