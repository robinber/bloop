#!/bin/sh
set -eu

identity="${BLOOP_SIGN_IDENTITY:-Bloop Dev}"
root="$(cd "$(dirname "$0")/.." && pwd)"
app="$root/target/release/Bloop.app"

cargo build --release --locked --package bloop-app --bin bloop-app --manifest-path "$root/Cargo.toml" --target-dir "$root/target"

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$root/bundle/Info.plist" "$app/Contents/Info.plist"
cp "$root/bundle/Bloop.icns" "$app/Contents/Resources/Bloop.icns"
version="$(cargo metadata --no-deps --format-version 1 --manifest-path "$root/Cargo.toml" |
    python3 -I -c 'import json, sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "bloop-app"))')"
plutil -replace CFBundleShortVersionString -string "$version" "$app/Contents/Info.plist"
cp "$root/target/release/bloop-app" "$app/Contents/MacOS/Bloop"

codesign --force --sign "$identity" "$app"
codesign --verify --strict "$app"

echo "$app"
