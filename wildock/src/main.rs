//! Wildock — native Aurora dock.
//!
//! Reads `/etc/wilos/dock.toml` (overridable by `~/.config/wildock/
//! dock.toml`) for its pinned items and renders a layer-shell window
//! anchored to the bottom edge of the focused monitor. Items
//! magnify on hover via a CSS transform spring; clicking spawns the
//! configured `exec` command.
//!
//! v0: pinned-only icons, no live taskbar yet (foreign-toplevel-
//! management binding to come). Until parity, the live ISO keeps
//! Waybar's dock instance — wildock is opt-in via `WILOS_USE_WILDOCK`.

use gtk::glib;
use gtk::prelude::*;
use gtk::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, CssProvider,
    Image, Label, Orientation, gdk::Display,
    style_context_add_provider_for_display, STYLE_PROVIDER_PRIORITY_APPLICATION,
};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use serde::Deserialize;
use std::{fs, path::PathBuf, process::Command};

const APP_ID: &str = "org.wilos.Wildock";
const SYSTEM_CONFIG: &str = "/etc/wilos/dock.toml";

const STYLE: &str = r#"
window#wildock {
    background: rgba(20, 20, 30, 0.55);
    border: 1px solid rgba(255, 255, 255, 0.10);
    border-radius: 22px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.45),
                inset 0 1px 0 rgba(255, 255, 255, 0.06);
}
.wildock-button {
    background: transparent;
    border: none;
    border-radius: 14px;
    padding: 4px 8px;
    margin: 0 2px;
    transition: all 220ms cubic-bezier(0.16, 1.0, 0.3, 1.0);
}
.wildock-button:hover {
    background: rgba(255, 255, 255, 0.10);
    margin-bottom: 8px;
}
.wildock-sep {
    color: rgba(255, 255, 255, 0.18);
    padding: 0 6px;
}
"#;

#[derive(Debug, Deserialize, Clone)]
struct DockEntry {
    label: String,
    icon: String,
    exec: String,
    #[serde(default)]
    sep_after: bool,
}

#[derive(Debug, Deserialize, Default)]
struct DockConfig {
    #[serde(default)]
    items: Vec<DockEntry>,
}

fn config_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        out.push(PathBuf::from(home).join(".config/wildock/dock.toml"));
    }
    out.push(PathBuf::from(SYSTEM_CONFIG));
    out
}

fn load_config() -> DockConfig {
    for path in config_paths() {
        if let Ok(raw) = fs::read_to_string(&path) {
            match toml::from_str(&raw) {
                Ok(cfg) => return cfg,
                Err(e) => eprintln!("wildock: {}: {}", path.display(), e),
            }
        }
    }
    eprintln!("wildock: no config found, using built-in defaults");
    default_config()
}

fn default_config() -> DockConfig {
    DockConfig {
        items: vec![
            DockEntry { label: "Files".into(),     icon: "org.gnome.Nautilus".into(),
                        exec: "nautilus".into(),  sep_after: false },
            DockEntry { label: "Launchpad".into(), icon: "view-grid-symbolic".into(),
                        exec: "wofi --show drun".into(), sep_after: false },
            DockEntry { label: "Browser".into(),   icon: "firefox".into(),
                        exec: "firefox".into(),   sep_after: false },
            DockEntry { label: "Terminal".into(),  icon: "kitty".into(),
                        exec: "kitty".into(),     sep_after: true },
            DockEntry { label: "Trash".into(),     icon: "user-trash-symbolic".into(),
                        exec: "nautilus trash:///".into(), sep_after: false },
        ],
    }
}

fn spawn(cmd: &str) {
    if cmd.is_empty() {
        return;
    }
    if let Err(e) = Command::new("/bin/sh").args(["-c", cmd]).spawn() {
        eprintln!("wildock: spawn '{cmd}' failed: {e}");
    }
}

fn build_ui(app: &Application) {
    let cfg = load_config();

    let window = ApplicationWindow::builder()
        .application(app)
        .build();
    window.set_widget_name("wildock");

    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.auto_exclusive_zone_enable();
    window.set_anchor(Edge::Bottom, true);
    window.set_margin(Edge::Bottom, 12);

    let row = GtkBox::new(Orientation::Horizontal, 4);
    row.set_halign(Align::Center);
    row.set_margin_start(14);
    row.set_margin_end(14);
    row.set_margin_top(6);
    row.set_margin_bottom(6);

    for entry in &cfg.items {
        let btn = Button::new();
        btn.add_css_class("wildock-button");
        btn.set_tooltip_text(Some(&entry.label));

        let icon = Image::from_icon_name(&entry.icon);
        icon.set_pixel_size(40);
        btn.set_child(Some(&icon));

        let cmd = entry.exec.clone();
        btn.connect_clicked(move |_| spawn(&cmd));
        row.append(&btn);

        if entry.sep_after {
            let sep = Label::new(Some("│"));
            sep.add_css_class("wildock-sep");
            row.append(&sep);
        }
    }

    window.set_child(Some(&row));
    window.present();
}

fn load_css() {
    let provider = CssProvider::new();
    provider.load_from_data(STYLE);
    if let Some(display) = Display::default() {
        style_context_add_provider_for_display(
            &display,
            &provider,
            STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn main() -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| load_css());
    app.connect_activate(build_ui);
    app.run()
}
