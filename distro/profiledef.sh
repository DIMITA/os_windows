#!/usr/bin/env bash
# WilOS archiso profile definition.
# This file is sourced by `mkarchiso`.

# shellcheck disable=SC2034

iso_name="wilos"
iso_label="WILOS_$(date +%Y%m)"
iso_publisher="WilOS <https://wilos.example>"
iso_application="WilOS Live/Install Medium"
iso_version="$(date +%Y.%m.%d)"
install_dir="wilos"
buildmodes=('iso')
bootmodes=(
    'bios.syslinux.mbr'
    'bios.syslinux.eltorito'
    'uefi-ia32.grub.esp'
    'uefi-x64.grub.esp'
    'uefi-ia32.grub.eltorito'
    'uefi-x64.grub.eltorito'
)
arch="x86_64"
pacman_conf="pacman.conf"
airootfs_image_type="squashfs"
airootfs_image_tool_options=('-comp' 'zstd' '-Xcompression-level' '20' '-b' '1M')
bootstrap_tarball_compression=('zstd' '-c' '-T0' '--long' '--auto-threads=logical' '-')
file_permissions=(
    ["/etc/shadow"]="0:0:400"
    ["/etc/gshadow"]="0:0:400"
    ["/root"]="0:0:750"
    ["/root/.automated_script.sh"]="0:0:755"
    ["/usr/local/bin/wilos-install"]="0:0:755"
    ["/usr/local/bin/wilos-firstrun"]="0:0:755"
)
