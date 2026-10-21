#!/usr/bin/env bash
# Wilai uninstaller. Nukes everything the installer put down.
#
#   curl -fsSL https://raw.githubusercontent.com/dimita/os_windows/main/scripts/uninstall.sh | sudo sh
#
# What it removes (in order):
#   1. wilai-daemon.service + wilai-first-models.service units
#   2. The `wilai` package (binaries, tool catalogue, systemd units)
#   3. /var/lib/wilai  (state, memory, audit, rollback snapshots)
#   4. /etc/wilai      (config)
#   5. /run/wilai      (runtime dir)
#   6. /etc/ssh/sshd_config.d/50-wilai.conf
#   7. The `wilai` system user and group
#   8. (opt-in) Ollama + its models  — set WILAI_PURGE_OLLAMA=1
#   9. (opt-in) voice models under /var/lib/wilai/models — nuked with 3.
#
# Flags:
#   WILAI_KEEP_STATE=1   keep /var/lib/wilai (memory, audit, rollback)
#   WILAI_KEEP_CONFIG=1  keep /etc/wilai (your custom config)
#   WILAI_PURGE_OLLAMA=1 also uninstall Ollama + delete its models
#   WILAI_YES=1          don't ask for confirmation
#
# Idempotent: safe to re-run.

set -euo pipefail

is_tty() { [ -t 1 ]; }
if is_tty; then
    C_BLUE='\033[1;34m'; C_GREEN='\033[1;32m'; C_YELLOW='\033[1;33m'
    C_RED='\033[1;31m';  C_RST='\033[0m'
else
    C_BLUE=''; C_GREEN=''; C_YELLOW=''; C_RED=''; C_RST=''
fi
say()  { printf "${C_BLUE}▸${C_RST} %s\n" "$*"; }
ok()   { printf "${C_GREEN}✓${C_RST} %s\n" "$*"; }
warn() { printf "${C_YELLOW}!${C_RST} %s\n" "$*" >&2; }

[ "$(id -u)" -eq 0 ] || { echo "run with sudo:  curl … | sudo sh" >&2; exit 1; }

cat <<EOF

${C_RED}This will remove Wilai from this machine.${C_RST}

  Removed:
    * wilai-daemon.service, wilai-first-models.service
    * /usr/bin/wilai, wilai-daemon, wilai-shell, wilai-hud, wilai-voice, wilai-overlay
    * /usr/share/wilai (tool catalogue)
    * $( [ "${WILAI_KEEP_STATE:-0}"  = 1 ] && echo "/var/lib/wilai KEPT (WILAI_KEEP_STATE=1)"  || echo "/var/lib/wilai  (memory, audit, rollback snapshots)" )
    * $( [ "${WILAI_KEEP_CONFIG:-0}" = 1 ] && echo "/etc/wilai     KEPT (WILAI_KEEP_CONFIG=1)" || echo "/etc/wilai      (your config)" )
    * /run/wilai, sshd drop-in, wilai user + group
EOF

if [ "${WILAI_PURGE_OLLAMA:-0}" = 1 ]; then
    echo "    * Ollama binary + its models (WILAI_PURGE_OLLAMA=1)"
fi

echo

if [ "${WILAI_YES:-0}" != 1 ] && is_tty; then
    printf "${C_YELLOW}Proceed? [y/N] ${C_RST}"
    read -r ans
    case "$ans" in [yY]|[yY][eE][sS]) : ;; *) echo "cancelled."; exit 0 ;; esac
fi

#─── 1. stop services ────────────────────────────────────────────
say "Stopping services..."
systemctl stop  wilai-daemon.service          2>/dev/null || true
systemctl stop  wilai-first-models.service    2>/dev/null || true
systemctl disable wilai-daemon.service        2>/dev/null || true
systemctl disable wilai-first-models.service  2>/dev/null || true

#─── 2. purge wilai package (handles /usr/bin + units + /usr/share) ──
if dpkg -l 2>/dev/null | grep -q '^ii *wilai '; then
    say "Purging wilai package..."
    DEBIAN_FRONTEND=noninteractive apt-get purge -y wilai >/dev/null 2>&1 || \
        dpkg --purge wilai 2>/dev/null || true
fi
# Belt-and-braces for a half-removed state.
rm -f /usr/bin/wilai /usr/bin/wilai-daemon /usr/bin/wilai-shell \
      /usr/bin/wilai-hud /usr/bin/wilai-voice /usr/bin/wilai-overlay \
      /usr/bin/wilai-install /usr/bin/wilai-selfupdate \
      /usr/bin/wilai-remote /usr/bin/wilai-screencap /usr/bin/wilai-vision
rm -rf /usr/share/wilai
rm -f  /usr/lib/systemd/system/wilai-daemon.service \
       /etc/systemd/system/wilai-daemon.service \
       /etc/systemd/system/wilai-first-models.service \
       /usr/lib/systemd/user/wilai.service \
       /usr/lib/systemd/user/wilai-hud.service
rm -f  /etc/ssh/sshd_config.d/50-wilai.conf

#─── 3. state + 4. config + 5. runtime ───────────────────────────
if [ "${WILAI_KEEP_STATE:-0}" = 1 ]; then
    warn "keeping /var/lib/wilai (WILAI_KEEP_STATE=1)"
else
    say "Removing /var/lib/wilai (state, memory, audit, rollback)..."
    rm -rf /var/lib/wilai
fi

if [ "${WILAI_KEEP_CONFIG:-0}" = 1 ]; then
    warn "keeping /etc/wilai (WILAI_KEEP_CONFIG=1)"
else
    say "Removing /etc/wilai..."
    rm -rf /etc/wilai
fi

rm -rf /run/wilai

#─── 6. systemd reload after unit file deletions ─────────────────
systemctl daemon-reload || true
systemctl reload ssh 2>/dev/null || systemctl reload sshd 2>/dev/null || true

#─── 7. remove wilai user + group ────────────────────────────────
if id -u wilai >/dev/null 2>&1; then
    say "Removing wilai system user..."
    deluser wilai 2>/dev/null || userdel wilai 2>/dev/null || true
fi
if getent group wilai >/dev/null 2>&1; then
    delgroup wilai 2>/dev/null || groupdel wilai 2>/dev/null || true
fi

#─── 8. (opt-in) nuke Ollama ─────────────────────────────────────
if [ "${WILAI_PURGE_OLLAMA:-0}" = 1 ]; then
    say "Removing Ollama + its models (~several GB)..."
    systemctl stop ollama     2>/dev/null || true
    systemctl disable ollama  2>/dev/null || true
    rm -f  /etc/systemd/system/ollama.service
    rm -f  /usr/local/bin/ollama /usr/bin/ollama
    rm -rf /usr/share/ollama /var/lib/ollama /root/.ollama
    # Per-user ollama models (uses SUDO_USER's home if present).
    if [ -n "${SUDO_USER:-}" ] && [ -d "/home/${SUDO_USER}/.ollama" ]; then
        rm -rf "/home/${SUDO_USER}/.ollama"
    fi
    if id -u ollama >/dev/null 2>&1; then
        deluser ollama 2>/dev/null || userdel ollama 2>/dev/null || true
    fi
    if getent group ollama >/dev/null 2>&1; then
        delgroup ollama 2>/dev/null || groupdel ollama 2>/dev/null || true
    fi
    systemctl daemon-reload || true
    ok "ollama removed"
else
    warn "Ollama kept. To also remove it, re-run with WILAI_PURGE_OLLAMA=1."
fi

#─── done ────────────────────────────────────────────────────────
cat <<EOF

${C_GREEN}═══════════════════════════════════════════════════════════${C_RST}
${C_GREEN}  Wilai uninstalled.${C_RST}
${C_GREEN}═══════════════════════════════════════════════════════════${C_RST}

  To reinstall:
      ${C_BLUE}curl -fsSL https://raw.githubusercontent.com/dimita/os_windows/main/install.sh | sudo sh${C_RST}

EOF
