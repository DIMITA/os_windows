# Wilai distribution assets

Files used at packaging time and at first run.

## `systemd/wilai.service`

User-level unit. Install with:

```
mkdir -p ~/.config/systemd/user
cp dist/systemd/wilai.service ~/.config/systemd/user/wilai.service
systemctl --user daemon-reload
systemctl --user enable --now wilai.service
```

Or, in a system-wide install (the WilOS Aurora ISO drops it in
`/usr/lib/systemd/user/`), it auto-starts when the user logs in via
`graphical-session.target`.

The unit assumes:

- `wilai-daemon` is on `PATH` at `/usr/bin/wilai-daemon`.
- The user's audit log lives under `$XDG_DATA_HOME/wilai`
  (defaults to `~/.local/share/wilai`). The unit grants write access there.
- The socket lives in `$XDG_RUNTIME_DIR/wilai.sock`. Granted via `%t`.

`ProtectHome=read-only` plus the explicit `ReadWritePaths=%h/.local/share/wilai`
keeps the daemon out of the rest of $HOME unless tools the user has approved
write to it (each tool's executor gets the daemon's permissions; the
`ProtectHome` umbrella is the floor, not a per-tool gate).

## Hyprland keybinds (suggested)

```
# ~/.config/hypr/hyprland.conf
bind = SUPER, GRAVE,        exec, wilai-overlay --ask
bind = SUPER, F12,          exec, wilai-voice  # push-to-talk in a small terminal
```

## Voice engines

`wilai-voice` shells out to user-provided binaries. Recommended setup:

- **STT**: build `whisper.cpp` from source and place its CLI binary on `PATH`.
  Drop a model under `~/.local/share/wilai/models/whisper-*.bin`.
- **TTS**: install `piper`. Drop a voice model under
  `~/.local/share/wilai/models/piper-*.onnx`.
- **Wake word** (optional): run any external detector that prints a line on
  stdout when triggered, then pass `--wake-cmd "openwakeword …"` to
  `wilai-voice`. Without it, the binary runs in push-to-talk mode.
