# Wilbar

The native top bar for WilOS Aurora — written in Rust, GTK4, and
gtk4-layer-shell. Eventual replacement for the Waybar instance that
ships today.

## Status

**v0 scaffold.** This crate produces a binary that anchors a glass
slab to the top of the screen and shows a live clock. It is the
seed for an extensible module system. Until parity with Waybar is
reached (workspaces, taskbar, system tray, network, audio, battery,
clock with calendar), the live ISO continues to ship Waybar.

To opt into wilbar in the live session:

```sh
export WILOS_USE_WILBAR=1
hyprctl dispatch exec wilbar
```

## Build

```sh
sudo pacman -S rustup gtk4 gtk4-layer-shell
rustup default stable
cd wilbar
cargo build --release
sudo install -Dm755 target/release/wilbar /usr/local/bin/wilbar
```

## Roadmap (pre-1.0)

- [ ] Module registry, declarative TOML config
- [ ] Workspaces module driven by Hyprland IPC
- [ ] Taskbar module (foreign-toplevel-management protocol)
- [ ] StatusNotifierItem tray
- [ ] PipeWire volume + brightness
- [ ] Network + Bluetooth toggles
- [ ] Battery + power profile
- [ ] Calendar pop-out
- [ ] Drop Waybar from the ISO once parity is reached
