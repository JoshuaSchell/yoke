#!/usr/bin/env bash
# Renders assets/icon.svg into the app icons cargo-packager and the app use:
#   icon.icns (macOS bundle), icon.png (1024, Linux), icon-256.png (embedded, runtime),
#   icon.ico (Windows installer and exe).
# Needs macOS (iconutil) and Nix (librsvg, imagemagick).
set -euo pipefail
cd "$(dirname "$0")/../assets"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/icon.iconset"
nix shell nixpkgs#librsvg nixpkgs#imagemagick -c bash -c '
  set -euo pipefail
  for s in 16 32 128 256 512; do
    rsvg-convert -w $s -h $s icon.svg -o "$0/icon.iconset/icon_${s}x${s}.png"
    rsvg-convert -w $((s*2)) -h $((s*2)) icon.svg -o "$0/icon.iconset/icon_${s}x${s}@2x.png"
  done
  rsvg-convert -w 1024 -h 1024 icon.svg -o icon.png
  rsvg-convert -w 256 -h 256 icon.svg -o icon-256.png
  magick icon.png -define icon:auto-resize=256,128,64,48,32,16 icon.ico
' "$tmp"
iconutil -c icns "$tmp/icon.iconset" -o icon.icns
echo "wrote assets/icon.{icns,png,ico} and icon-256.png"
