#!/usr/bin/env bash
# Build the WilOS ISO from a NON-Arch host (Ubuntu/Debian/Fedora/macOS,
# anything with Docker or Podman). Uses the official Arch Linux image,
# installs archiso + rust inside, then delegates to scripts/build-iso.sh.
#
# Usage (from the repo root):
#   ./scripts/build-iso-docker.sh                  # output -> ./out/
#   ./scripts/build-iso-docker.sh /path/to/output
#
# Env knobs:
#   RUNTIME=docker|podman   (default: auto-detect, prefers podman)
#   IMAGE=archlinux:latest  (override the base image)
#
# The container is one-shot and auto-removed; the ISO lands in your
# host filesystem, not inside the container.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.."; pwd)"
OUT="${1:-$ROOT/out}"
mkdir -p "$OUT"

IMAGE="${IMAGE:-archlinux:latest}"
if [[ -z "${RUNTIME:-}" ]]; then
    if command -v podman >/dev/null; then
        RUNTIME=podman
    elif command -v docker >/dev/null; then
        RUNTIME=docker
    else
        echo "Neither podman nor docker is installed. Install one:" >&2
        echo "  Ubuntu/Debian: sudo apt install podman   # or docker.io" >&2
        exit 1
    fi
fi

# archiso needs a privileged container (loop devices + mount). On
# rootless podman that's fine; on docker you may need sudo.
PRIV_FLAGS=(--privileged)

echo ">> Runtime : $RUNTIME"
echo ">> Image   : $IMAGE"
echo ">> Repo    : $ROOT"
echo ">> Output  : $OUT"
echo

exec "$RUNTIME" run --rm -it \
    "${PRIV_FLAGS[@]}" \
    -v "$ROOT:/wilos:z" \
    -v "$OUT:/out:z" \
    -w /wilos \
    "$IMAGE" \
    bash -c '
set -euo pipefail
echo ">> Updating pacman + installing build deps..."
pacman -Syu --noconfirm archiso imagemagick rustup squashfs-tools \
                        git base-devel >/dev/null

echo ">> Installing Rust stable..."
rustup default stable >/dev/null

echo ">> Running build-iso.sh..."
cd /wilos
./scripts/build-iso.sh /out

echo
echo ">> Done. ISO files:"
ls -lh /out/*.iso 2>/dev/null || echo "  (no ISO produced — check logs above)"
'
