#!/usr/bin/env bash
# Build the WilOS Aurora live/installable ISO from the archiso profile.
#
# Requirements (run on an Arch Linux host):
#   sudo pacman -S archiso imagemagick
#
# Usage:
#   sudo ./scripts/build-iso.sh           # default output ./out/
#   sudo ./scripts/build-iso.sh /tmp/wilos
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.."; pwd)"
PROFILE="$ROOT/distro"
OUT="${1:-$ROOT/out}"
WORK="${OUT}/work"

if [[ $EUID -ne 0 ]]; then
    echo "build-iso.sh must run as root (mkarchiso needs it)."
    echo "Try: sudo $0 $*"
    exit 1
fi

if ! command -v mkarchiso >/dev/null; then
    echo "mkarchiso not found. Install with: pacman -S archiso"
    exit 1
fi

# Ensure the wallpaper exists; generate it if missing.
WALL="$PROFILE/airootfs/usr/share/backgrounds/wilos/aurora.jpg"
if [[ ! -f "$WALL" ]]; then
    echo "Generating Aurora wallpaper..."
    bash "$ROOT/scripts/make-wallpaper.sh"
fi

mkdir -p "$OUT" "$WORK"
echo "Building WilOS ISO..."
echo "  profile : $PROFILE"
echo "  work    : $WORK"
echo "  output  : $OUT"

mkarchiso -v -w "$WORK" -o "$OUT" "$PROFILE"

echo
echo "Done. ISO available in $OUT"
ls -lh "$OUT"/*.iso
