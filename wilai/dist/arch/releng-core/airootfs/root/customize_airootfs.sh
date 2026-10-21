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
install -d -m 0755 -o root -g root /var/lib/wilai/models
touch /var/lib/wilai/wizard-pending
chmod 0644 /var/lib/wilai/wizard-pending

# Marker file read by wilai-shell on first boot: tells users how to
# drop in a whisper STT model + a piper voice to enable voice I/O.
cat > /var/lib/wilai/models/README <<'EOF'
Drop a whisper.cpp model file here (e.g. ggml-base.en.bin) and a piper
voice (.onnx) to enable voice I/O in wilai-shell. Then set in
~/.config/wilai/config.toml:

  [voice]
  enabled       = true
  whisper_model = "ggml-base.en.bin"
  piper_model   = "en_GB-alan-medium.onnx"

Everything else is on PATH already (whisper-cli, piper, arecord, aplay).
EOF

# Clean the pacman cache so the ISO stays small.
pacman -Scc --noconfirm || true

# Permissions on the Wilai assets.
if [[ -d /usr/share/wilai ]]; then
    chmod -R a+rX /usr/share/wilai
fi
