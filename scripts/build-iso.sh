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

# Stage Wilai (system agent) into the airootfs. Built locally before
# mkarchiso so the live image carries the four binaries plus the YAML
# tool registry, the example config, and the systemd user unit.
stage_wilai() {
    if [[ ! -d "$ROOT/wilai" ]]; then
        echo "wilai/ missing; skipping wilai staging."
        return 0
    fi
    if ! command -v cargo >/dev/null; then
        echo "cargo not found; cannot build wilai. Install rustup."
        return 1
    fi
    echo "Building Wilai (release)..."
    ( cd "$ROOT/wilai" && cargo build --release --frozen 2>/dev/null \
        || cargo build --release )

    local BIN="$PROFILE/airootfs/usr/local/bin"
    local SHARE="$PROFILE/airootfs/usr/share/wilai"
    local UNIT="$PROFILE/airootfs/usr/lib/systemd/user"
    install -Dm755 "$ROOT/wilai/target/release/wilai"          "$BIN/wilai"
    install -Dm755 "$ROOT/wilai/target/release/wilai-daemon"   "$BIN/wilai-daemon"
    install -Dm755 "$ROOT/wilai/target/release/wilai-voice"    "$BIN/wilai-voice"
    install -Dm755 "$ROOT/wilai/target/release/wilai-overlay"  "$BIN/wilai-overlay"

    install -d "$SHARE/tools/core" "$SHARE/tools/pentest"
    install -Dm644 "$ROOT/wilai/tools/core/"*.yaml    "$SHARE/tools/core/"
    install -Dm644 "$ROOT/wilai/tools/pentest/"*.yaml "$SHARE/tools/pentest/"
    install -Dm644 "$ROOT/wilai/config/wilai.toml.example" \
        "$SHARE/wilai.toml.example"
    install -Dm644 "$ROOT/wilai/dist/systemd/wilai.service" \
        "$UNIT/wilai.service"

    # Auto-enable the user unit on the live ISO so a fresh boot has the
    # daemon ready as soon as Hyprland brings up graphical-session.target.
    install -d "$PROFILE/airootfs/etc/skel/.config/systemd/user/graphical-session.target.wants"
    ln -sf "/usr/lib/systemd/user/wilai.service" \
        "$PROFILE/airootfs/etc/skel/.config/systemd/user/graphical-session.target.wants/wilai.service"

    echo "Wilai staged into airootfs."
}
stage_wilai

mkdir -p "$OUT" "$WORK"
echo "Building WilOS ISO..."
echo "  profile : $PROFILE"
echo "  work    : $WORK"
echo "  output  : $OUT"

mkarchiso -v -w "$WORK" -o "$OUT" "$PROFILE"

echo
echo "Done. ISO available in $OUT"
ls -lh "$OUT"/*.iso
