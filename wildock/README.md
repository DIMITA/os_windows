# Wildock

Native Aurora dock for WilOS — Rust + GTK4 + gtk4-layer-shell.
Reads `/etc/wilos/dock.toml` (overridable by `~/.config/wildock/
dock.toml`) and renders pinned launcher icons anchored to the bottom
edge of the focused monitor.

## Status

**v0 scaffold.** Pinned-only icons. The live taskbar (foreign-
toplevel-management) is wired in v0.5. Until parity, the live ISO
keeps Waybar's dock instance — wildock is opt-in via
`WILOS_USE_WILDOCK=1`.

## Build

```sh
sudo pacman -S rustup gtk4 gtk4-layer-shell
cd wildock
cargo build --release
sudo install -Dm755 target/release/wildock /usr/local/bin/wildock
```

## Config

```toml
# /etc/wilos/dock.toml
[[items]]
label = "Files"
icon  = "org.gnome.Nautilus"
exec  = "nautilus"

[[items]]
label = "Browser"
icon  = "firefox"
exec  = "firefox"
sep_after = true
```
