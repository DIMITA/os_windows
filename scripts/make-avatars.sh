#!/usr/bin/env bash
# Generate the preset avatar set offered by the installer.
# Each avatar is a 256x256 PNG: a flat coloured disc with the WilOS
# aurora glyph layered on top. The avatars are intentionally simple
# so they read well in account chips and login bubbles.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.."; pwd)"
OUT="$ROOT/distro/airootfs/usr/share/wilos-branding/avatars"
SVG_BASE="$ROOT/distro/airootfs/usr/share/wilos-branding/wilos-logo.svg"
mkdir -p "$OUT"

if ! command -v rsvg-convert >/dev/null; then
    echo "rsvg-convert not installed; skipping avatar generation."
    exit 0
fi

declare -A bg=(
    [aurora]='#1B1442'
    [ocean]='#0E2F4A'
    [forest]='#0E3B2C'
    [sunset]='#3B0E2A'
    [graphite]='#1A1A22'
    [ember]='#3A1208'
)

for name in "${!bg[@]}"; do
    color="${bg[$name]}"
    tmp="$(mktemp --suffix=.svg)"
    sed "s|#1B1442|$color|g; s|#100B2A|$color|g; s|#06030C|$color|g" \
        "$SVG_BASE" > "$tmp"
    rsvg-convert -w 256 -h 256 "$tmp" -o "$OUT/$name.png"
    rm -f "$tmp"
done

echo "Wrote ${#bg[@]} avatars to $OUT"
ls -1 "$OUT"
