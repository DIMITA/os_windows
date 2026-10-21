#!/usr/bin/env bash
# Runs inside the airootfs during image build (archiso convention). Any
# changes made here land in the shipped filesystem.
set -euo pipefail

# Timezone + locale defaults; the wizard will reset these on first boot.
ln -sf /usr/share/zoneinfo/UTC /etc/localtime
echo 'en_US.UTF-8 UTF-8' > /etc/locale.gen
locale-gen
echo 'LANG=en_US.UTF-8' > /etc/locale.conf

# Hostname — a placeholder, Wilai changes it in step 3 of the wizard.
echo 'wilos' > /etc/hostname

# Enable the services WilOS-Core needs on first boot.
systemctl enable NetworkManager.service
systemctl enable systemd-timesyncd.service
systemctl enable ollama.service
systemctl enable wilai-daemon.service
systemctl enable wilai-shell.service
# Mask the first getty so the shell owns tty1.
systemctl mask getty@tty1.service || true

# Audit log + memory DB directories.
install -d -m 0750 -o root -g root /var/lib/wilai
touch /var/lib/wilai/wizard-pending
chmod 0644 /var/lib/wilai/wizard-pending

# Clean the pacman cache so the ISO stays small.
pacman -Scc --noconfirm || true

# Permissions on the Wilai assets.
if [[ -d /usr/share/wilai ]]; then
    chmod -R a+rX /usr/share/wilai
fi
