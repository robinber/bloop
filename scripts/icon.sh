#!/bin/sh
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

qlmanage -t -s 1024 -o "$work" "$root/assets/branding/app-icon.svg" >/dev/null
master="$work/app-icon.svg.png"
test -f "$master"

iconset="$work/Bloop.iconset"
mkdir "$iconset"
for size in 16 32 128 256 512; do
    sips -z "$size" "$size" "$master" --out "$iconset/icon_${size}x${size}.png" >/dev/null
    double=$((size * 2))
    sips -z "$double" "$double" "$master" --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil --convert icns --output "$root/bundle/Bloop.icns" "$iconset"

echo "$root/bundle/Bloop.icns"
