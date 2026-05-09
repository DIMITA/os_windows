#!/usr/bin/env bash
# Generate the default WilOS Aurora wallpaper.
#
# Recipe: a deep navy-to-indigo radial base, with two soft aurora
# light bands (cyan + magenta) painted on top using screen compositing,
# then a faint noise texture so blur surfaces stay alive.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.."; pwd)"
OUT_DIR="$ROOT/distro/airootfs/usr/share/backgrounds/wilos"
OUT="$OUT_DIR/aurora.jpg"
W=2560
H=1440

mkdir -p "$OUT_DIR"

CONVERT=""
command -v magick  >/dev/null && CONVERT="magick"
command -v convert >/dev/null && CONVERT="${CONVERT:-convert}"
if [[ -z "$CONVERT" ]]; then
    echo "ImageMagick not installed; skipping wallpaper generation."
    echo "(The compositor will fall back to background_color.)"
    exit 0
fi

# 1. Base: dark radial gradient (deep indigo at the center, near-black edges).
$CONVERT -size "${W}x${H}" \
    radial-gradient:'#1A1342'-'#06030C' \
    "$OUT_DIR/_base.png"

# Helper: build a circular blob of given hex color, scaled & offset.
# A 1.6x oversized canvas ensures the gradient really fades to black
# before it hits the wallpaper edges.
make_blob() {
    local hex="$1" off="$2" out="$3" mod="${4:-100,100,100}"
    local CW=$((W * 16 / 10)) CH=$((H * 16 / 10))
    $CONVERT -size "${CW}x${CH}" \
        radial-gradient:"${hex}"-'#000000' \
        -gravity center -crop "${W}x${H}+0+0" +repage \
        -roll "$off" \
        -modulate "$mod" \
        "$out"
}

# 2. Cyan aurora blob, top-left.
make_blob '#3B7CFF' '+400-300' "$OUT_DIR/_band1.png" '38,100,100'
# 3. Magenta aurora blob, bottom-right.
make_blob '#B05CFF' '-500+250' "$OUT_DIR/_band2.png" '32,100,100'
# 4. Faint pink highlight, far top-right.
make_blob '#FF8AC8' '-700-450' "$OUT_DIR/_band3.png" '22,80,100'

# 5. Composite (screen blend so we add light without washing out).
$CONVERT "$OUT_DIR/_base.png" \
    "$OUT_DIR/_band1.png" -compose screen -composite \
    "$OUT_DIR/_band2.png" -compose screen -composite \
    "$OUT_DIR/_band3.png" -compose screen -composite \
    -modulate 100,115,100 \
    "$OUT_DIR/_blend.png"

# 6. Subtle film grain so glass surfaces have something to blur.
$CONVERT "$OUT_DIR/_blend.png" \
    \( -size "${W}x${H}" xc:'gray(50%)' \
       +noise gaussian -channel A -evaluate set 6% +channel \) \
    -compose softlight -composite \
    -strip -quality 88 -interlace Plane \
    "$OUT"

rm -f "$OUT_DIR"/_base.png "$OUT_DIR"/_band1.png "$OUT_DIR"/_band2.png \
      "$OUT_DIR"/_band3.png "$OUT_DIR"/_blend.png

echo "Wallpaper written to $OUT ($(du -h "$OUT" | cut -f1))"
