#!/usr/bin/env bash
# Wilai — one-line installer. Zero manual commands after this.
#
#   curl -fsSL https://raw.githubusercontent.com/dimita/os_windows/main/install.sh | sudo sh
#
# Works on: Ubuntu 22.04+, Debian 12+, Linux Mint, Pop!_OS, Elementary.
# Takes care of EVERYTHING end-to-end: apt deps, Ollama install, LLM
# model downloads (foreground — you see progress), .deb install, group
# membership, daemon start, verification.
#
# Opt-in extras (flip the env var to any value):
#   WILAI_WITH_VISION=1    also pulls llava:7b (~4.5 GB) for screen vision
#   WILAI_WITH_VOICE=1     installs whisper.cpp + piper-tts binaries +
#                          downloads a whisper base model and a piper
#                          English voice (~400 MB total). Enables voice
#                          in the default config.
#
# Other knobs:
#   WILAI_NO_MODELS=1      skip LLM model pulls (point to a remote Ollama)
#   WILAI_VERSION=v1.2.3   pin a specific release
#   WILAI_REPO=fork/x      install from a fork
#
# Idempotent: re-running upgrades Wilai and tops up models without
# touching your config, state, memory, or audit log.

set -euo pipefail

#─── pretty output ────────────────────────────────────────────────
is_tty() { [ -t 1 ]; }
if is_tty; then
    C_BLUE='\033[1;34m'; C_GREEN='\033[1;32m'; C_YELLOW='\033[1;33m'
    C_RED='\033[1;31m';  C_DIM='\033[2m';     C_RST='\033[0m'
else
    C_BLUE=''; C_GREEN=''; C_YELLOW=''; C_RED=''; C_DIM=''; C_RST=''
fi
say()  { printf "${C_BLUE}▸${C_RST} %s\n" "$*"; }
ok()   { printf "${C_GREEN}✓${C_RST} %s\n" "$*"; }
warn() { printf "${C_YELLOW}!${C_RST} %s\n" "$*" >&2; }
die()  { printf "${C_RED}✗${C_RST} %s\n" "$*" >&2; exit 1; }

trap 'ec=$?; printf "\n${C_RED}✗${C_RST} install.sh died at line ${LINENO} (exit ${ec}). Scroll up for the last message, or re-run with:\n  curl -fsSL ... | sudo bash -x\n" >&2' ERR

#─── preflight ────────────────────────────────────────────────────
[ "$(id -u)" -eq 0 ] || die "run this with sudo:  curl -fsSL … | sudo sh"

OS=$(. /etc/os-release && echo "$ID")
case "$OS" in
    ubuntu|debian|linuxmint|pop|elementary) : ;;
    *) die "unsupported OS: $OS. Supported: Ubuntu, Debian, Mint, Pop!_OS, Elementary." ;;
esac

ARCH=$(dpkg --print-architecture)
case "$ARCH" in
    amd64|arm64) : ;;
    *) die "unsupported arch: $ARCH" ;;
esac

say "Wilai installer — $OS/$ARCH"
say "This takes ~15 min on a fresh system (most of it is the 4 GB LLM download)."
echo

#─── 1. apt deps (small, fast) ───────────────────────────────────
say "Installing system dependencies..."
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq \
    curl ca-certificates \
    libxkbcommon0 \
    network-manager pipewire pipewire-pulse alsa-utils \
    openssh-server socat python3 \
    >/dev/null
ok "apt deps installed"

#─── 2. Ollama (local LLM runtime) ───────────────────────────────
if command -v ollama >/dev/null 2>&1; then
    ok "ollama already present ($(ollama --version 2>&1 | head -1))"
else
    say "Installing Ollama (local LLM runtime)..."
    curl -fsSL https://ollama.com/install.sh | sh >/dev/null 2>&1 || true
    systemctl enable --now ollama >/dev/null 2>&1 || true
    ollama_up=0
    for _ in $(seq 1 30); do
        if curl -fsS http://127.0.0.1:11434/api/tags >/dev/null 2>&1; then
            ollama_up=1; break
        fi
        sleep 1
    done
    if [ "$ollama_up" = 1 ]; then
        ok "ollama installed and running"
    else
        die "ollama installed but not answering on 127.0.0.1:11434 — try: sudo systemctl status ollama"
    fi
fi

#─── 3. Pull LLM models (foreground, user sees progress) ─────────
if [ "${WILAI_NO_MODELS:-0}" = 0 ]; then
    say "Pulling mistral:7b-instruct (~4 GB — Ollama shows its own progress bar)..."
    sudo -u "${SUDO_USER:-root}" ollama pull mistral:7b-instruct
    ok "mistral:7b-instruct ready"

    say "Pulling nomic-embed-text (~250 MB)..."
    sudo -u "${SUDO_USER:-root}" ollama pull nomic-embed-text
    ok "nomic-embed-text ready"

    if [ "${WILAI_WITH_VISION:-0}" = 1 ]; then
        say "Pulling llava:7b (~4.5 GB — opt-in via WILAI_WITH_VISION=1)..."
        sudo -u "${SUDO_USER:-root}" ollama pull llava:7b
        ok "llava:7b ready"
    fi
else
    warn "WILAI_NO_MODELS=1 — skipping LLM model pulls."
fi

#─── 4. Wilai .deb (prebuilt from GitHub Releases) ───────────────
REPO="${WILAI_REPO:-dimita/os_windows}"
if [ -n "${WILAI_VERSION:-}" ]; then
    REL_API="https://api.github.com/repos/${REPO}/releases/tags/${WILAI_VERSION}"
else
    REL_API="https://api.github.com/repos/${REPO}/releases/latest"
fi

say "Looking for a prebuilt Wilai release..."
REL_JSON=$(mktemp)
HTTP=$(curl -sS -o "$REL_JSON" -w "%{http_code}" "$REL_API" || echo 000)
DEB_URL=""
if [ "$HTTP" = 200 ]; then
    DEB_URL=$(python3 -c "
import json, re
with open('$REL_JSON') as f: rel = json.load(f)
for a in rel.get('assets', []):
    if re.search(r'wilai_.*_${ARCH}\.deb\$', a['name']):
        print(a['browser_download_url']); break
" 2>/dev/null || true)
fi
rm -f "$REL_JSON"

if [ -z "$DEB_URL" ]; then
    if [ "$HTTP" = 404 ]; then
        warn "no tagged release on GitHub yet — falling back to source build."
    else
        warn "no prebuilt .deb for ${ARCH} on this release (HTTP $HTTP) — building from source."
    fi
    warn "source build needs ~5 min + ~2 GB RAM."
    FALLBACK_SOURCE=1
else
    FALLBACK_SOURCE=0
fi

if [ "$FALLBACK_SOURCE" = 1 ]; then
    say "Installing build dependencies..."
    apt-get install -y -qq \
        git build-essential pkg-config dpkg-dev fakeroot \
        libxkbcommon-dev libwayland-dev libssl-dev \
        >/dev/null
    if ! command -v cargo >/dev/null 2>&1; then
        say "Installing Rust (one-time, ~1 min)..."
        curl --proto '=https' --tlsv1.2 -fsSL https://sh.rustup.rs \
            | sh -s -- -y --default-toolchain stable --profile minimal >/dev/null
        export PATH="/root/.cargo/bin:$PATH"
    fi
    SRC=$(mktemp -d)
    say "Cloning repo + building (grab a coffee, ~5 min)..."
    git clone --depth 1 "https://github.com/${REPO}.git" "$SRC" >/dev/null 2>&1
    cd "$SRC/wilai"
    ./scripts/build-deb --release
    DEB=$(ls dist/debian/out/wilai_*_${ARCH}.deb | head -1)
else
    say "Downloading $(basename "$DEB_URL")..."
    DEB=$(mktemp --suffix=.deb)
    curl -fsSL "$DEB_URL" -o "$DEB"
fi

say "Installing Wilai..."
apt-get install -y -qq "$DEB" >/dev/null
ok "wilai installed"

#─── 5. Compatibility shim: shell/daemon socket path mismatch ────
# Older daemons (shipped in v1.0.0) put the socket at
# /run/wilai/wilai.sock; older shells look for /run/wilai.sock. The
# fix is in the code already, but for the deb that is currently on
# the user's machine right now, drop a symlink so the default path
# works out of the box.
if [ -S /run/wilai/wilai.sock ] && [ ! -e /run/wilai.sock ]; then
    ln -sf /run/wilai/wilai.sock /run/wilai.sock
    chmod 0660 /run/wilai/wilai.sock 2>/dev/null || true
fi

#─── 6. Add invoking user to the wilai group ─────────────────────
# The .deb postinst already does this via SUDO_USER, but if the
# installer was launched any other way (bare root, sudo -E stripping
# SUDO_USER, etc.) catch it here. Idempotent.
if [ -n "${SUDO_USER:-}" ] && id "$SUDO_USER" >/dev/null 2>&1; then
    if ! id -nG "$SUDO_USER" | grep -qw wilai; then
        say "Adding $SUDO_USER to the wilai group..."
        usermod -aG wilai "$SUDO_USER" || true
    fi
fi

#─── 7. (opt-in) voice stack — whisper.cpp + piper-tts ───────────
if [ "${WILAI_WITH_VOICE:-0}" = 1 ]; then
    say "Installing voice stack (opt-in via WILAI_WITH_VOICE=1)..."
    apt-get install -y -qq pipewire-audio libatomic1 >/dev/null

    install -d -o wilai -g wilai -m 0750 /var/lib/wilai/models

    # whisper.cpp prebuilt binary from the latest release.
    if ! command -v whisper-cli >/dev/null 2>&1; then
        WHISPER_URL=$(curl -fsSL "https://api.github.com/repos/ggml-org/whisper.cpp/releases/latest" \
            | python3 -c "import json,sys,re
r=json.load(sys.stdin)
for a in r.get('assets',[]):
    if re.search(r'linux-x64.*\.tar\.gz\$', a['name']):
        print(a['browser_download_url']); break" || true)
        if [ -n "$WHISPER_URL" ]; then
            tmp=$(mktemp -d)
            curl -fsSL "$WHISPER_URL" | tar -xz -C "$tmp"
            find "$tmp" -name 'whisper-cli' -executable -exec install -Dm755 {} /usr/local/bin/whisper-cli \;
            rm -rf "$tmp"
            ok "whisper-cli installed"
        else
            warn "could not fetch whisper-cli release — voice disabled."
        fi
    fi

    # whisper model (base.en, ~140 MB).
    if [ ! -f /var/lib/wilai/models/ggml-base.en.bin ]; then
        say "Downloading whisper base.en model (~140 MB)..."
        curl -fsSL -o /var/lib/wilai/models/ggml-base.en.bin \
            https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin
        chown wilai:wilai /var/lib/wilai/models/ggml-base.en.bin
    fi

    # piper-tts prebuilt binary.
    if ! command -v piper >/dev/null 2>&1; then
        PIPER_URL=$(curl -fsSL "https://api.github.com/repos/rhasspy/piper/releases/latest" \
            | python3 -c "import json,sys,re
r=json.load(sys.stdin)
for a in r.get('assets',[]):
    if re.search(r'piper_linux_x86_64.*\.tar\.gz\$', a['name']):
        print(a['browser_download_url']); break" || true)
        if [ -n "$PIPER_URL" ]; then
            tmp=$(mktemp -d)
            curl -fsSL "$PIPER_URL" | tar -xz -C "$tmp"
            install -Dm755 "$tmp/piper/piper" /usr/local/bin/piper
            cp -r "$tmp/piper/espeak-ng-data" /usr/local/share/espeak-ng-data 2>/dev/null || true
            rm -rf "$tmp"
            ok "piper installed"
        else
            warn "could not fetch piper release — voice disabled."
        fi
    fi

    # piper voice (en_US-amy-medium, ~63 MB).
    if [ ! -f /var/lib/wilai/models/en_US-amy-medium.onnx ]; then
        say "Downloading piper voice en_US-amy-medium (~63 MB)..."
        base=https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/amy/medium
        curl -fsSL -o /var/lib/wilai/models/en_US-amy-medium.onnx      "$base/en_US-amy-medium.onnx"
        curl -fsSL -o /var/lib/wilai/models/en_US-amy-medium.onnx.json "$base/en_US-amy-medium.onnx.json"
        chown wilai:wilai /var/lib/wilai/models/en_US-amy-medium.onnx*
    fi

    # Enable voice in the system config.
    if [ -f /etc/wilai/config.toml ]; then
        if ! grep -q "^\[voice\]" /etc/wilai/config.toml; then
            cat >> /etc/wilai/config.toml <<'EOF'

[voice]
enabled = true
stt_model = "/var/lib/wilai/models/ggml-base.en.bin"
tts_voice = "/var/lib/wilai/models/en_US-amy-medium.onnx"
EOF
        fi
    fi
    ok "voice stack installed and enabled"
    warn "If you're in VirtualBox: on the HOST, VM Settings → Audio → enable 'Audio Input'."
fi

#─── 8. Restart daemon so it picks up any config change ──────────
systemctl restart wilai-daemon.service || true
sleep 1

#─── 9. Verify the daemon is live ────────────────────────────────
say "Verifying daemon..."
for _ in $(seq 1 10); do
    if systemctl is-active --quiet wilai-daemon.service; then
        ok "wilai-daemon.service active"
        break
    fi
    sleep 1
done

#─── done ────────────────────────────────────────────────────────
cat <<EOF

${C_GREEN}═══════════════════════════════════════════════════════════${C_RST}
${C_GREEN}  Wilai is installed and ready.${C_RST}
${C_GREEN}═══════════════════════════════════════════════════════════${C_RST}

  ${C_BLUE}LAST STEP${C_RST} — pick up the wilai group in your current shell:

      ${C_BLUE}newgrp wilai${C_RST}

  (Or just log out and log back in once.)
  Then launch:

      ${C_BLUE}wilai-shell${C_RST}

  Inside the TUI, type ${C_BLUE}/help${C_RST} to see what you can ask.

${C_DIM}  Logs:    journalctl -u wilai-daemon -f${C_RST}
${C_DIM}  Models:  ollama list${C_RST}
${C_DIM}  Uninstall: curl -fsSL https://raw.githubusercontent.com/${REPO}/main/scripts/uninstall.sh | sudo sh${C_RST}
EOF
