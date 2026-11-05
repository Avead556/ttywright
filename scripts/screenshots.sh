#!/bin/sh
set -eu

cd "$(dirname "$0")/.."
out=docs/screenshots
chrome=${CHROME:-"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"}

cargo build --quiet --bin ttyw
cargo run --quiet --example screenshots -- "$out"

for page in "$out"/*.html; do
    "$chrome" --headless=new --disable-gpu --hide-scrollbars \
        --force-device-scale-factor=2 --default-background-color=00000000 \
        --virtual-time-budget=8000 --window-size=1340,950 \
        --screenshot="$PWD/${page%.html}.png" "file://$PWD/$page" >/dev/null 2>&1
    rm "$page"
done
ls "$out"
