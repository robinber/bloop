#!/bin/sh
set -eu

identity="${BLOOP_SIGN_IDENTITY:-Bloop Dev}"
root="$(cd "$(dirname "$0")/.." && pwd)"
app="$root/target/release/Bloop.app"

cargo build --release --package bloop-app --manifest-path "$root/Cargo.toml"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS"
cp "$root/bundle/Info.plist" "$app/Contents/Info.plist"
cp "$root/target/release/bloop-app" "$app/Contents/MacOS/Bloop"

codesign --force --sign "$identity" "$app"
codesign --verify --strict "$app"

echo "$app"
