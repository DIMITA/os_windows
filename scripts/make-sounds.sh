#!/usr/bin/env bash
# Generate the WilOS Aurora sound design — short synthesised chimes
# encoded as Ogg Vorbis so they ship inside the ISO without legal
# baggage.
#
# - notification.oga : single soft bell ding (~200 ms)
# - login.oga       : 4-note ascending arpeggio (~900 ms)
# - logout.oga      : 4-note descending arpeggio (~900 ms)
# - error.oga       : two-tone short alert (~250 ms)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.."; pwd)"
OUT="$ROOT/distro/airootfs/usr/share/sounds/wilos"
mkdir -p "$OUT"

if ! command -v sox >/dev/null || ! command -v oggenc >/dev/null; then
    echo "sox + oggenc required; skipping sound generation."
    exit 0
fi

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# Helper: synthesise N notes (frequencies in $@), each `dur` seconds,
# blended with a short fade and a touch of reverb, to $tmp/$out.wav.
synth() {
    local out="$1" dur="$2"; shift 2
    local files=()
    local i=0
    for f in "$@"; do
        local part="$tmp/${out}-${i}.wav"
        sox -n "$part" \
            synth "$dur" sine "$f" sine "$(awk -v f="$f" 'BEGIN{print f*2}')" \
            channels 2 \
            vol 0.55 \
            fade t 0.005 "$dur" 0.18
        files+=("$part")
        i=$((i + 1))
    done
    sox "${files[@]}" -b 16 -e signed-integer "$tmp/${out}.wav" \
        gain -h \
        reverb 25 50 100 100 0 0 \
        norm -3
}

# notification: a short C5 + E5 dyad.
synth notification 0.18 523.25 659.25
oggenc -q 6 -o "$OUT/notification.oga" "$tmp/notification.wav" >/dev/null

# login: ascending C5 → E5 → G5 → B5 (Cmaj7 voicing).
synth login 0.20 523.25 659.25 783.99 987.77
oggenc -q 6 -o "$OUT/login.oga" "$tmp/login.wav" >/dev/null

# logout: descending arpeggio.
synth logout 0.18 987.77 783.99 659.25 523.25
oggenc -q 6 -o "$OUT/logout.oga" "$tmp/logout.wav" >/dev/null

# error: low minor second (A4 + Bb4) — short and dry.
synth error 0.12 440.00 466.16
oggenc -q 6 -o "$OUT/error.oga" "$tmp/error.wav" >/dev/null

ls -lh "$OUT"
