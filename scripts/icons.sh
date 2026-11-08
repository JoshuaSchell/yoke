#!/usr/bin/env bash
# Renders assets/Yoke.icon (an Icon Composer document: a white glass ox's head on black)
# into the app icons cargo-packager and the app use:
#   icon.icns (macOS bundle), icon.png (1024, Linux), icon-256.png (embedded, runtime),
#   icon.ico (Windows installer and exe).
# Needs macOS with Icon Composer (its `ictool` renders the glass; set ICTOOL to point at it
# if it isn't in /Applications or a mounted Icon Composer disk image), and Nix (imagemagick,
# for the .ico).
set -euo pipefail
cd "$(dirname "$0")/../assets"
ictool=${ICTOOL:-}
if [ -z "$ictool" ]; then
  for c in "/Applications/Icon Composer.app" /Volumes/Icon\ Composer*/"Icon Composer.app"; do
    if [ -x "$c/Contents/Executables/ictool" ]; then ictool="$c/Contents/Executables/ictool"; break; fi
  done
fi
[ -x "$ictool" ] || { echo "ictool not found: install Icon Composer, or set ICTOOL" >&2; exit 1; }
magick() { nix shell nixpkgs#imagemagick -c magick "$@"; }
# ictool lights the squircle's edge as glass whatever the document says; the fill is solid black,
# so everything within 4.5% of the edge is painted black again, keeping the shape's alpha.
render() {
  "$ictool" Yoke.icon --export-image --output-file "$1" --platform macOS --rendition Default --width "$2" --height "$2" --scale 1 >/dev/null
  local r=$(( $2 * 45 / 1000 + 1 ))
  magick "$1" -write mpr:src +delete \
    mpr:src -alpha off \( mpr:src -alpha extract -virtual-pixel black -morphology Erode "Disk:$r" \) -compose multiply -composite \
    \( mpr:src -alpha extract \) -compose copy_opacity -composite "$1"
}
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir "$tmp/icon.iconset"
for s in 16 32 128 256 512; do
  render "$tmp/icon.iconset/icon_${s}x${s}.png" $s
  render "$tmp/icon.iconset/icon_${s}x${s}@2x.png" $((s*2))
done
render icon.png 1024
render icon-256.png 256
iconutil -c icns "$tmp/icon.iconset" -o icon.icns
magick icon.png -define icon:auto-resize=256,128,64,48,32,16 icon.ico
echo "wrote assets/icon.{icns,png,ico} and icon-256.png"
