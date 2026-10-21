#!/usr/bin/env bash
# archiso profile for WilOS-Core: a headless-first Linux where Wilai IS
# the interface. No desktop, no X/Wayland. Boots to tty1 and launches
# wilai-shell at login.
#
# Build with:
#   sudo mkarchiso -v -w /tmp/wilos-core-work -o out dist/arch/releng-core/
# shellcheck disable=SC2034

iso_name="wilos-core"
iso_label="WILOS_CORE_$(date +%Y%m)"
iso_publisher="WilOS <https://github.com/dimita/os_windows>"
iso_application="WilOS-Core Live/Install ISO"
iso_version="$(date +%Y.%m.%d)"
install_dir="arch"
buildmodes=('iso')
bootmodes=('bios.syslinux' 'uefi.grub')
arch="x86_64"
pacman_conf="pacman.conf"
airootfs_image_type="squashfs"
airootfs_image_tool_options=('-comp' 'zstd' '-Xcompression-level' '20' '-b' '1M')
file_permissions=(
  ["/etc/shadow"]="0:0:400"
  ["/root"]="0:0:750"
  ["/root/.automated_script.sh"]="0:0:755"
  ["/root/customize_airootfs.sh"]="0:0:755"
  ["/var/lib/wilai"]="0:0:755"
  ["/var/lib/wilai/wizard-pending"]="0:0:644"
)
