#!/usr/bin/env bash
# Flash the WilOS ISO onto a USB drive. Mandatory typed confirmation.
set -euo pipefail

ISO="${1:-}"
DEV="${2:-}"

usage() {
    echo "Usage: sudo $0 <iso> <device>"
    echo "Example: sudo $0 out/wilos-2026.05.09-x86_64.iso /dev/sdb"
    exit 1
}

[[ -z "$ISO" || -z "$DEV" ]] && usage
[[ -f "$ISO" ]] || { echo "ISO not found: $ISO"; exit 1; }
[[ -b "$DEV" ]] || { echo "Not a block device: $DEV"; exit 1; }
[[ $EUID -eq 0 ]] || { echo "Must run as root."; exit 1; }

# Refuse anything that looks like an internal disk.
if [[ "$DEV" =~ /dev/(sda|nvme0n1|mmcblk0)$ ]]; then
    echo "Refusing $DEV — looks like an internal disk."
    echo "If you really want to flash an internal disk, use dd manually."
    exit 1
fi

echo
echo "About to OVERWRITE every byte on $DEV with $ISO."
echo "Existing data on $DEV will be lost."
echo
echo "Device summary:"
lsblk -o NAME,SIZE,MODEL,VENDOR,TRAN "$DEV"
echo

read -rp "Type 'FLASH ${DEV##*/}' to continue: " token
[[ "$token" == "FLASH ${DEV##*/}" ]] || { echo "Cancelled."; exit 0; }

echo "Writing... (this can take a few minutes)"
dd if="$ISO" of="$DEV" bs=4M status=progress conv=fsync oflag=direct
sync
echo
echo "Done. You may now boot from $DEV."
