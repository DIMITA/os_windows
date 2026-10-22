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
# Default LLM model: qwen2.5:3b (~2 GB RAM, strong multilingual, good
# at tool-call JSON). Fits on an 8 GB system with room to spare, but
# needs 4+ CPU cores to feel snappy on CPU-only (no GPU).
#
# Low-end hardware? Two escape hatches:
#
#   WILAI_SMALL_MODEL=1       use qwen2.5:0.5b (~500 MB, lightning fast
#                             even on 1 vCPU — tool-call quality drops
#                             a bit, but totally usable for sys.*).
#   WILAI_REMOTE_OLLAMA=URL   skip the local Ollama install entirely.
#                             Wilai points at a remote Ollama endpoint
#                             you already run on a stronger machine
#                             (homelab, LAN desktop, VPS). The local
#                             .deb + config get installed, no models
#                             pulled, no RAM/CPU pressure for LLM
#                             inference. Example:
#                               WILAI_REMOTE_OLLAMA=http://192.168.1.42:11434
#
# Opt-in extras (flip the env var to any value):
#   WILAI_WITH_MISTRAL=1   also pulls mistral:7b-instruct (~4.5 GB) for
#                          higher-quality responses — needs 12+ GB RAM
#                          and 4+ CPUs or a GPU to stay snappy.
#   WILAI_WITH_VISION=1    also pulls llava:7b (~4.5 GB) for screen vision
#   WILAI_WITH_VOICE=1     installs whisper.cpp + piper-tts binaries +
#                          downloads a whisper base model and a piper
#                          English voice (~400 MB total). Enables voice
#                          in the default config.
#   WILAI_QUANT=q3         use an aggressive-quant variant of the default
#                          model (e.g. qwen2.5:3b-instruct-q3_K_M). +25%
#                          inference speed, ~5% quality drop. Default
#                          is q4_K_M.
#   WILAI_OLLAMA_NATIVE=1  compile Ollama from source with -march=native
#                          so llama.cpp picks up THIS CPU's SIMD
#                          extensions (AVX2/AVX-512/AVX-VNNI/AMX).
#                          +10-30% speed on modern CPUs. Install takes
#                          ~15 min extra; the binary is pinned to this
#                          host's CPU and must not be copied to another.
#
# Other knobs:
#   WILAI_NO_MODELS=1      skip LLM model pulls (point to a remote Ollama)
#   WILAI_VERSION=v1.2.3   pin a specific release
#   WILAI_REPO=fork/x      install from a fork
#
# Idempotent: re-running upgrades Wilai and tops up models without
# touching your config, state, memory, or audit log.

set -eu

#─── pretty output ────────────────────────────────────────────────
# Store the actual ESC byte in each variable so `cat <<EOF`
# interpolates it correctly. Writing them as '\033[…]' strings only
# works when passed through printf — cat/here-docs print them raw.
is_tty() { [ -t 1 ]; }
if is_tty; then
    ESC=$(printf '\033')
    C_BLUE="${ESC}[1;34m"; C_GREEN="${ESC}[1;32m"; C_YELLOW="${ESC}[1;33m"
    C_RED="${ESC}[1;31m";  C_DIM="${ESC}[2m";     C_RST="${ESC}[0m"
else
    C_BLUE=''; C_GREEN=''; C_YELLOW=''; C_RED=''; C_DIM=''; C_RST=''
fi
say()  { printf "${C_BLUE}▸${C_RST} %s\n" "$*"; }
ok()   { printf "${C_GREEN}✓${C_RST} %s\n" "$*"; }
warn() { printf "${C_YELLOW}!${C_RST} %s\n" "$*" >&2; }
die()  { printf "${C_RED}✗${C_RST} %s\n" "$*" >&2; exit 1; }

# POSIX-portable failure trap. ERR is bash-only; using EXIT instead
# lets us run under /bin/sh (dash on Ubuntu/Debian).
WILAI_OK=0
_wilai_cleanup() {
    rc=$?
    if [ "$WILAI_OK" != 1 ] && [ "$rc" != 0 ]; then
        printf "\n%bx%b install.sh exited with code %s. Scroll up for the last message, or re-run with:\n  curl -fsSL ... | sudo sh -x\n" \
            "$C_RED" "$C_RST" "$rc" >&2
    fi
}
trap _wilai_cleanup EXIT

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

# Pick the default LLM. Precedence: WILAI_SMALL_MODEL overrides base
# choice; WILAI_QUANT picks a quantization suffix. Ollama exposes
# quants as separate tags, e.g. qwen2.5:3b-instruct-q3_K_M.
DEFAULT_MODEL="qwen2.5:3b"
if [ "${WILAI_SMALL_MODEL:-0}" = 1 ]; then
    DEFAULT_MODEL="qwen2.5:0.5b"
    say "WILAI_SMALL_MODEL=1 — using $DEFAULT_MODEL (fast, lower quality)."
fi
case "${WILAI_QUANT:-}" in
    q3)
        # qwen2.5:3b-instruct-q3_K_M = ~1.7 GB, ~25% faster than default
        # q4, slight quality drop. Only wired for the 3b/0.5b default
        # chain for now (same base model, different quant tag).
        if [ "$DEFAULT_MODEL" = "qwen2.5:3b" ]; then
            DEFAULT_MODEL="qwen2.5:3b-instruct-q3_K_M"
        elif [ "$DEFAULT_MODEL" = "qwen2.5:0.5b" ]; then
            DEFAULT_MODEL="qwen2.5:0.5b-instruct-q3_K_M"
        fi
        say "WILAI_QUANT=q3 — using $DEFAULT_MODEL (+25% speed, -5% quality)."
        ;;
    q4|"") : ;;  # default
    *)
        warn "WILAI_QUANT='$WILAI_QUANT' not recognized (q3 or q4); ignoring."
        ;;
esac

#─── 2. Ollama (local LLM runtime) ───────────────────────────────
if [ -n "${WILAI_REMOTE_OLLAMA:-}" ]; then
    say "Using remote Ollama at $WILAI_REMOTE_OLLAMA — not installing a local one."
    # Verify we can reach the remote so we don't ship a broken config.
    if ! curl -fsS "${WILAI_REMOTE_OLLAMA%/}/api/tags" >/dev/null 2>&1; then
        die "cannot reach $WILAI_REMOTE_OLLAMA/api/tags — check the host is reachable and listening on 0.0.0.0"
    fi
    ok "remote Ollama reachable"
elif command -v ollama >/dev/null 2>&1; then
    ok "ollama already present ($(ollama --version 2>&1 | head -1))"
elif [ "${WILAI_OLLAMA_NATIVE:-0}" = 1 ]; then
    # Compile Ollama from source with -march=native so the bundled
    # llama.cpp picks up every SIMD extension THIS CPU exposes
    # (AVX2, AVX-512, AVX-VNNI, AMX). +10-30% inference speed on
    # modern CPUs. Takes ~10-15 min extra at install, needs Go 1.21+
    # and cmake + a C++ toolchain. Binary is pinned to this CPU
    # micro-architecture — don't copy it to a different host.
    say "WILAI_OLLAMA_NATIVE=1 — compiling Ollama from source with -march=native..."
    apt-get install -y -qq golang-go cmake build-essential git >/dev/null
    tmp=$(mktemp -d)
    git clone --depth 1 https://github.com/ollama/ollama.git "$tmp" >/dev/null 2>&1
    (
        cd "$tmp"
        export CMAKE_C_FLAGS="-march=native -O3"
        export CMAKE_CXX_FLAGS="-march=native -O3"
        go generate ./... >/dev/null 2>&1 || true
        go build -o ollama . >/dev/null
    )
    install -m 0755 "$tmp/ollama" /usr/local/bin/ollama
    rm -rf "$tmp"
    # systemd unit — same shape as the upstream installer's one,
    # with the GUI-friendly CPU budget baked in.
    if ! id ollama >/dev/null 2>&1; then
        useradd -r -s /bin/false -U -d /usr/share/ollama ollama || true
    fi
    install -d -o ollama -g ollama /usr/share/ollama
    _cpus=$(nproc 2>/dev/null || echo 2)
    if [ "$_cpus" -le 2 ]; then _threads=1
    elif [ "$_cpus" -le 4 ]; then _threads=$((_cpus - 1))
    else _threads=4
    fi
    cat > /etc/systemd/system/ollama.service <<EOF
[Unit]
Description=Ollama (native build via WILAI_OLLAMA_NATIVE=1)
After=network-online.target

[Service]
ExecStart=/usr/local/bin/ollama serve
User=ollama
Group=ollama
Restart=always
Nice=10
CPUWeight=50
Environment="HOME=/usr/share/ollama"
Environment="OLLAMA_NUM_THREAD=$_threads"

[Install]
WantedBy=multi-user.target
EOF
    systemctl daemon-reload
    systemctl enable --now ollama >/dev/null 2>&1 || true
    ollama_up=0
    for _ in $(seq 1 30); do
        if curl -fsS http://127.0.0.1:11434/api/tags >/dev/null 2>&1; then
            ollama_up=1; break
        fi
        sleep 1
    done
    if [ "$ollama_up" = 1 ]; then
        ok "ollama (native build) running"
    else
        die "ollama compiled but not answering on 127.0.0.1:11434 — try: sudo systemctl status ollama"
    fi
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

#─── 2b. CPU budget for Ollama — keep the GUI responsive ─────────
# On a 2-vCPU VirtualBox VM, Ollama inference at 100% on both cores
# starves the host from drawing the guest GUI. Even a 14 GB VM
# freezes visually because the issue isn't memory, it's CPU
# scheduling. Give Ollama lower priority + a thread cap so one core
# is always free for the compositor + wilai-shell. Users can
# override by editing the drop-in.
# Skip when WILAI_REMOTE_OLLAMA — the local box has no Ollama to
# throttle, inference pressure lives on the remote host.
if [ -z "${WILAI_REMOTE_OLLAMA:-}" ]; then
    cpus=$(nproc 2>/dev/null || echo 2)
    # Leave 1 core for the GUI; cap at 4 so bigger hosts don't give
    # all 32 threads to Ollama (that spikes power + heat without
    # helping inference on a bandwidth-bound workload).
    if [ "$cpus" -le 2 ]; then
        threads=1
    elif [ "$cpus" -le 4 ]; then
        threads=$((cpus - 1))
    else
        threads=4
    fi
    install -d /etc/systemd/system/ollama.service.d
    cat > /etc/systemd/system/ollama.service.d/wilai-cpu.conf <<EOF
# Wilai-managed CPU budget. Keeps the host GUI responsive by
# leaving at least one core free. Tweak freely.
[Service]
Nice=10
CPUWeight=50
Environment="OLLAMA_NUM_THREAD=$threads"
EOF
    systemctl daemon-reload
    systemctl restart ollama >/dev/null 2>&1 || true
    ok "ollama CPU budget: Nice=10, OLLAMA_NUM_THREAD=$threads (of $cpus cpus)"
fi

#─── 3. Pull LLM models (foreground, user sees progress) ─────────
# Skip when using a remote Ollama (models live on the remote).
if [ -n "${WILAI_REMOTE_OLLAMA:-}" ]; then
    say "Remote Ollama — assuming $DEFAULT_MODEL + nomic-embed-text are already pulled there."
elif [ "${WILAI_NO_MODELS:-0}" = 0 ]; then
    say "Pulling $DEFAULT_MODEL (Ollama shows its own progress bar)..."
    sudo -u "${SUDO_USER:-root}" ollama pull "$DEFAULT_MODEL"
    ok "$DEFAULT_MODEL ready (default LLM)"

    say "Pulling nomic-embed-text (~250 MB)..."
    sudo -u "${SUDO_USER:-root}" ollama pull nomic-embed-text
    ok "nomic-embed-text ready"

    if [ "${WILAI_WITH_MISTRAL:-0}" = 1 ]; then
        say "Pulling mistral:7b-instruct (~4.5 GB — opt-in via WILAI_WITH_MISTRAL=1)..."
        sudo -u "${SUDO_USER:-root}" ollama pull mistral:7b-instruct
        ok "mistral:7b-instruct ready"
    fi

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

DEB_URL=""
if [ "${WILAI_FROM_SOURCE:-0}" = 1 ]; then
    warn "WILAI_FROM_SOURCE=1 — skipping release lookup, building from main."
    FALLBACK_SOURCE=1
else
    say "Looking for a prebuilt Wilai release..."
    REL_JSON=$(mktemp)
    HTTP=$(curl -sS -o "$REL_JSON" -w "%{http_code}" "$REL_API" || echo 000)
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

    # Cap cargo parallelism based on available RAM. rustc in release
    # mode can peak at ~3-4 GB per job on large crates (sqlcipher,
    # rustls, the various async stacks); with Ollama already running
    # it's easy to OOM a small VM. Rule of thumb: 1 job per ~3 GB
    # of total RAM, clamped to [1, nproc].
    total_mb=$(awk '/^MemTotal:/ {print int($2/1024)}' /proc/meminfo 2>/dev/null || echo 2048)
    cpus=$(nproc 2>/dev/null || echo 2)
    jobs=$(( total_mb / 3000 ))
    [ "$jobs" -lt 1 ] && jobs=1
    [ "$jobs" -gt "$cpus" ] && jobs=$cpus
    export CARGO_BUILD_JOBS=$jobs
    say "Build parallelism: $jobs job(s) (RAM ${total_mb} MB, CPUs ${cpus})."

    # Add 2 GB of swap on the fly if the VM has < 6 GB RAM AND no
    # swap yet — rustc linking in particular is swap-tolerant.
    swap_mb=$(awk '/^SwapTotal:/ {print int($2/1024)}' /proc/meminfo 2>/dev/null || echo 0)
    if [ "$total_mb" -lt 6000 ] && [ "$swap_mb" -lt 1000 ]; then
        say "Adding 2 GB temporary swap at /swap-wilai-install (low-RAM VM)..."
        fallocate -l 2G /swap-wilai-install 2>/dev/null \
            || dd if=/dev/zero of=/swap-wilai-install bs=1M count=2048 status=none
        chmod 600 /swap-wilai-install
        mkswap /swap-wilai-install >/dev/null 2>&1
        swapon /swap-wilai-install 2>/dev/null || true
    fi

    SRC=$(mktemp -d)
    say "Cloning repo + building (grab a coffee, 5-15 min)..."
    git clone --depth 1 "https://github.com/${REPO}.git" "$SRC" >/dev/null 2>&1
    cd "$SRC/wilai"
    ./scripts/build-deb --release
    DEB=$(ls dist/debian/out/wilai_*_${ARCH}.deb | head -1)

    # Drop the temporary swap if we added it.
    if [ -f /swap-wilai-install ]; then
        swapoff /swap-wilai-install 2>/dev/null || true
        rm -f /swap-wilai-install
    fi
else
    say "Downloading $(basename "$DEB_URL")..."
    DEB=$(mktemp --suffix=.deb)
    curl -fsSL "$DEB_URL" -o "$DEB"
fi

say "Installing Wilai..."
apt-get install -y -qq "$DEB" >/dev/null
ok "wilai installed"

#─── 4b. Patch config for WILAI_SMALL_MODEL / WILAI_REMOTE_OLLAMA ──
# Must run before the config seeding below if the config doesn't
# exist yet, so we leave a note and finalize it after the seed step.

#─── 5. Seed default config if none exists (v1.0.0 .deb bug) ────
# The v1.0.0 .deb postinst did not create /etc/wilai/config.toml,
# so the daemon tripped on empty defaults — prompts went nowhere.
# Fixed in the next release's postinst; back-fill it here for
# anyone on v1.0.0.
install -d -m 0755 /etc/wilai
if [ ! -f /etc/wilai/config.toml ] && [ -f /usr/share/wilai/wilai.toml.example ]; then
    install -m 0644 /usr/share/wilai/wilai.toml.example /etc/wilai/config.toml
    say "seeded /etc/wilai/config.toml from the shipped example"
fi
# Make it visible to the daemon's XDG resolver too (which looks under
# /var/lib/wilai/.config/wilai/ since that's $HOME for the wilai user).
install -d -o wilai -g wilai -m 0750 /var/lib/wilai/.config/wilai
if [ -f /etc/wilai/config.toml ] && [ ! -e /var/lib/wilai/.config/wilai/wilai.toml ]; then
    ln -sf /etc/wilai/config.toml /var/lib/wilai/.config/wilai/wilai.toml
fi

# Apply model + remote overrides. Idempotent, re-runnable.
if [ -f /etc/wilai/config.toml ]; then
    # Model switch: WILAI_SMALL_MODEL=1 → qwen2.5:0.5b. Also persist
    # it on re-runs so an upgrade installer with the flag updates the
    # config in place.
    sed -i "s|^default_model.*|default_model     = \"${DEFAULT_MODEL}\"|" /etc/wilai/config.toml
    # Remote Ollama endpoint.
    if [ -n "${WILAI_REMOTE_OLLAMA:-}" ]; then
        sed -i "s|^url   = \"http://127.0.0.1:11434\"|url   = \"${WILAI_REMOTE_OLLAMA}\"|" /etc/wilai/config.toml
        ok "config: pointed [providers.ollama] at $WILAI_REMOTE_OLLAMA"
    fi
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

#─── 9. Wait for the socket, then compat-symlink + chmod ─────────
# Must run AFTER the daemon restart. The daemon creates
# /run/wilai/wilai.sock on bind; both v1.0.0 and current shells
# look at it, but older shells also look at /run/wilai.sock, and
# the v1.0.0 daemon bound the socket 0600 which locked the group
# out. Belt + braces: wait up to 10s for the socket, drop the
# compat symlink, and reopen group perms.
say "Waiting for daemon socket..."
sock_up=0
for _ in $(seq 1 20); do
    if [ -S /run/wilai/wilai.sock ]; then
        sock_up=1; break
    fi
    sleep 0.5
done

if [ "$sock_up" = 1 ]; then
    chmod 0660 /run/wilai/wilai.sock 2>/dev/null || true
    [ -e /run/wilai.sock ] || ln -sf /run/wilai/wilai.sock /run/wilai.sock
    ok "socket at /run/wilai/wilai.sock (perms 0660, symlinked to /run/wilai.sock)"
else
    warn "/run/wilai/wilai.sock did not appear in 10s — see: sudo journalctl -u wilai-daemon"
fi

#─── 10. Verify the daemon is live ───────────────────────────────
say "Verifying daemon..."
for _ in $(seq 1 10); do
    if systemctl is-active --quiet wilai-daemon.service; then
        ok "wilai-daemon.service active"
        break
    fi
    sleep 1
done

#─── done ────────────────────────────────────────────────────────
WILAI_OK=1
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
