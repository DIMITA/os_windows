#!/usr/bin/env bash
# Wilai — one-line installer.
#
#   curl -fsSL https://raw.githubusercontent.com/dimita/os_windows/main/install.sh | sh
#
# Works on: Ubuntu 22.04+, Debian 12+, Linux Mint, Pop!_OS.
# Does: install Ollama (if missing) → install Wilai .deb (prebuilt
# from the latest GitHub release) → pull the two LLM models in the
# background → verify the daemon is up. No Rust toolchain required
# on the user's machine; everything is already compiled.
#
# Non-interactive by default. Set WILAI_NO_MODELS=1 to skip model
# pulls. Set WILAI_VERSION=v1.2.3 to pin an older release.
#
# This script is idempotent — re-running it upgrades Wilai to the
# newest release without touching your config, state, memory or
# audit log.

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

# Loud failure trap. Without this, `set -e` kills the script silently
# and the user is left wondering what happened.
trap 'ec=$?; printf "\n${C_RED}✗${C_RST} install.sh died at line ${LINENO} (exit ${ec}). Scroll up for the last message, or re-run with:\n  curl -fsSL ... | sudo bash -x\n" >&2' ERR

#─── preflight ────────────────────────────────────────────────────
[ "$(id -u)" -eq 0 ] || die "run this with sudo:  curl -fsSL … | sudo sh"

OS=$(. /etc/os-release && echo "$ID")
case "$OS" in
    ubuntu|debian|linuxmint|pop) : ;;
    *) die "unsupported OS: $OS. Supported: Ubuntu, Debian, Mint, Pop!_OS." ;;
esac

ARCH=$(dpkg --print-architecture)
case "$ARCH" in
    amd64|arm64) : ;;
    *) die "unsupported arch: $ARCH" ;;
esac

say "Wilai installer — $OS/$ARCH"

#─── 1. apt deps (small, fast) ───────────────────────────────────
say "Installing system dependencies..."
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq \
    curl ca-certificates \
    libxkbcommon0 \
    network-manager pipewire pipewire-pulse \
    openssh-server socat python3 \
    >/dev/null
ok "apt deps installed"

#─── 2. Ollama (local LLM runtime) ───────────────────────────────
if command -v ollama >/dev/null 2>&1; then
    ok "ollama already present ($(ollama --version 2>&1 | head -1))"
else
    say "Installing Ollama (local LLM runtime)..."
    # Ollama's installer sometimes exits non-zero on harmless warnings
    # (e.g. "useradd: group ollama exists") even when Ollama is in fact
    # installed. Don't let that kill the whole install — we verify
    # Ollama is actually answering below.
    curl -fsSL https://ollama.com/install.sh | sh >/dev/null 2>&1 || true
    systemctl enable --now ollama >/dev/null 2>&1 || true
    # Wait for it to answer, up to 30 seconds.
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
        die "ollama installed but is not answering on 127.0.0.1:11434. Try: sudo systemctl status ollama"
    fi
fi

#─── 3. Wilai .deb (prebuilt from GitHub Releases) ───────────────
REPO="${WILAI_REPO:-dimita/os_windows}"
if [ -n "${WILAI_VERSION:-}" ]; then
    REL_API="https://api.github.com/repos/${REPO}/releases/tags/${WILAI_VERSION}"
else
    REL_API="https://api.github.com/repos/${REPO}/releases/latest"
fi

say "Looking for a prebuilt Wilai release..."
# Fetch the releases API without -f so a 404 doesn't blow up curl;
# write the HTTP code separately and only parse JSON on a 200.
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
    say "Cloning repo + building (grab a coffee)..."
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

#─── 4. Pull the two default models (in background) ──────────────
if [ "${WILAI_NO_MODELS:-0}" = 0 ]; then
    say "Pulling default models — this runs in the background (~4 GB)."
    # Use a one-shot systemd unit so model pulls survive the install
    # script finishing. The user can watch progress with:
    #   journalctl -u wilai-first-models.service -f
    cat > /etc/systemd/system/wilai-first-models.service <<'EOF'
[Unit]
Description=Wilai: pull default Ollama models (mistral:7b-instruct + nomic-embed-text)
Wants=ollama.service
After=ollama.service network-online.target

[Service]
Type=oneshot
ExecStart=/bin/sh -c '/usr/bin/ollama pull mistral:7b-instruct && /usr/bin/ollama pull nomic-embed-text'
TimeoutStartSec=60min
# Delete self after success so the unit doesn't linger in `systemctl status`.
ExecStartPost=/bin/sh -c 'rm -f /etc/systemd/system/wilai-first-models.service && systemctl daemon-reload'
EOF
    systemctl daemon-reload
    systemctl start --no-block wilai-first-models.service
    ok "model pulls queued (watch with: journalctl -u wilai-first-models -f)"
else
    warn "skipping model pulls (WILAI_NO_MODELS=1). Pull manually with:"
    warn "  ollama pull mistral:7b-instruct && ollama pull nomic-embed-text"
fi

#─── 5. Verify the daemon is live ────────────────────────────────
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

${C_GREEN}Wilai is installed.${C_RST}

  Launch it with:          ${C_BLUE}wilai-shell${C_RST}
  Help inside the shell:   type ${C_BLUE}/help${C_RST}
  Watch model downloads:   ${C_DIM}journalctl -u wilai-first-models -f${C_RST}
  Daemon logs:             ${C_DIM}journalctl -u wilai-daemon -f${C_RST}

${C_DIM}The models (~4 GB) finish downloading in the background. Your first
prompt will work as soon as that unit prints "Deactivated successfully"
in the journal.${C_RST}
EOF
