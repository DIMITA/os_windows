//! Wilbar — native Aurora top bar.
//!
//! Layout (left → right):
//!   [logo] [workspaces] [window title]   [clock]   [audio] [bat] [net] [bright] [tray-stub]
//!
//! Modules implemented in this revision:
//!   • clock        — local time, refreshed every second
//!   • workspaces   — Hyprland IPC (UNIX socket events.sock)
//!   • window       — focused window title via Hyprland IPC
//!   • audio        — `wpctl get-volume @DEFAULT_AUDIO_SINK@`
//!   • battery      — `/sys/class/power_supply/BAT*/`
//!   • brightness   — `/sys/class/backlight/*/brightness`
//!   • network      — `nmcli -t -f STATE,CONNECTION g status` parsing
//!
//! Modules to come (v0.5):
//!   • taskbar      — wlr-foreign-toplevel-management
//!   • tray         — StatusNotifierItem D-Bus

use chrono::Local;
use gtk::glib;
use gtk::prelude::*;
use gtk::{
    Align, Application, ApplicationWindow, Box as GtkBox, Button, CssProvider,
    Label, Orientation, gdk::Display,
    style_context_add_provider_for_display, STYLE_PROVIDER_PRIORITY_APPLICATION,
};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use std::{
    fs,
    io::{BufRead, BufReader, Read},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::Command,
    thread,
    time::Duration,
};

const APP_ID: &str = "org.wilos.Wilbar";

const STYLE: &str = r#"
* {
    font-family: "Inter", "JetBrainsMono Nerd Font", sans-serif;
    font-size: 13px;
    font-weight: 500;
}
window#wilbar {
    background: rgba(15, 15, 23, 0.55);
    color: #ECEEF6;
    border-radius: 14px;
    border: 1px solid rgba(255, 255, 255, 0.08);
}
.wilbar-pad   { padding: 0 10px; }
.wilbar-clock {
    color: #FFFFFF;
    font-weight: 600;
    padding: 0 14px;
    background: rgba(255, 255, 255, 0.04);
    border-radius: 10px;
}
.wilbar-ws {
    color: rgba(236, 238, 246, 0.55);
    padding: 0 6px;
    background: transparent;
    border: none;
}
.wilbar-ws:hover  { color: #FFFFFF; background: rgba(255, 255, 255, 0.08); border-radius: 8px; }
.wilbar-ws-active { color: #7CC8FF; }
.wilbar-window    { color: rgba(236, 238, 246, 0.85); font-weight: 600; padding: 0 10px; }
.wilbar-status    { padding: 0 8px; }
.wilbar-status-warn  { color: #FFD580; }
.wilbar-status-crit  { color: #FF8A8A; }
.wilbar-wilai {
    padding: 0 10px;
    border-radius: 10px;
    color: rgba(236, 238, 246, 0.85);
    background: rgba(255, 255, 255, 0.04);
}
.wilbar-wilai-pentest {
    color: #FFFFFF;
    background: rgba(255, 90, 90, 0.30);
    border: 1px solid rgba(255, 138, 138, 0.55);
}
.wilbar-wilai-down { color: rgba(236, 238, 246, 0.30); }
"#;

// ---------- Hyprland IPC -------------------------------------------------- //

fn hypr_socket(kind: &str) -> Option<PathBuf> {
    // /run/user/<uid>/hypr/<HYPRLAND_INSTANCE_SIGNATURE>/<kind>.sock
    let xdg = std::env::var_os("XDG_RUNTIME_DIR")?;
    let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    Some(PathBuf::from(xdg).join("hypr").join(sig).join(kind))
}

fn hyprctl(cmd: &str) -> Option<String> {
    let out = Command::new("hyprctl").args(["-j", cmd]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

// Run a closure on the GTK thread for every event line emitted by
// Hyprland's events socket.
fn watch_hypr_events<F>(mut on_event: F)
where
    F: FnMut(&str, &str) + Send + 'static,
{
    let Some(path) = hypr_socket("socket2.sock") else { return };
    let (tx, rx) = std::sync::mpsc::channel::<String>();

    thread::spawn(move || {
        loop {
            let stream = match UnixStream::connect(&path) {
                Ok(s) => s,
                Err(_) => {
                    thread::sleep(Duration::from_secs(2));
                    continue;
                }
            };
            let reader = BufReader::new(stream);
            for line in reader.lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    return;
                }
            }
            thread::sleep(Duration::from_secs(1));
        }
    });

    glib::idle_add_local(move || {
        while let Ok(line) = rx.try_recv() {
            if let Some((evt, data)) = line.split_once(">>") {
                on_event(evt, data);
            }
        }
        glib::ControlFlow::Continue
    });
}

// ---------- modules ------------------------------------------------------- //

fn build_clock(parent: &GtkBox) {
    let lbl = Label::new(Some(""));
    lbl.add_css_class("wilbar-clock");
    let lbl_cl = lbl.clone();
    let tick = move || lbl_cl.set_label(&Local::now().format("%a %d %b   %H:%M").to_string());
    tick();
    let update = move || tick();
    let lbl_t = lbl.clone();
    glib::timeout_add_local(Duration::from_secs(1), move || {
        lbl_t.set_label(&Local::now().format("%a %d %b   %H:%M").to_string());
        glib::ControlFlow::Continue
    });
    parent.append(&lbl);
    let _ = update;
}

fn build_workspaces(parent: &GtkBox) {
    let row = GtkBox::new(Orientation::Horizontal, 2);
    parent.append(&row);

    let row_for_refresh = row.clone();
    let refresh = move || {
        // Wipe and rebuild from the JSON listing.
        while let Some(c) = row_for_refresh.first_child() {
            row_for_refresh.remove(&c);
        }
        let active_id = hyprctl("activeworkspace")
            .and_then(|s| extract_int_field(&s, "\"id\":"))
            .unwrap_or(1);
        let workspaces = hyprctl("workspaces").unwrap_or_default();
        let ids = parse_workspace_ids(&workspaces);
        let mut shown = if ids.is_empty() { vec![1] } else { ids };
        shown.sort();
        for id in shown {
            let btn = Button::with_label(&id.to_string());
            btn.add_css_class("wilbar-ws");
            if id == active_id {
                btn.add_css_class("wilbar-ws-active");
            }
            btn.connect_clicked(move |_| {
                let _ = Command::new("hyprctl")
                    .args(["dispatch", "workspace", &id.to_string()])
                    .status();
            });
            row_for_refresh.append(&btn);
        }
    };

    refresh();

    // Update on Hyprland events.
    let refresh_event = std::rc::Rc::new(refresh);
    let r = refresh_event.clone();
    watch_hypr_events(move |evt, _data| {
        if matches!(evt, "workspace" | "createworkspace" | "destroyworkspace"
                       | "focusedmon" | "activespecial") {
            r();
        }
    });
}

// Quick-and-dirty JSON extraction so we can avoid a serde_json dep.
fn extract_int_field(json: &str, key: &str) -> Option<i64> {
    let i = json.find(key)?;
    let rest = &json[i + key.len()..];
    let trimmed = rest.trim_start();
    let end = trimmed.find(|c: char| !c.is_ascii_digit() && c != '-')?;
    trimmed[..end].parse().ok()
}

fn parse_workspace_ids(json: &str) -> Vec<i64> {
    let mut out = Vec::new();
    let mut rest = json;
    while let Some(i) = rest.find("\"id\":") {
        rest = &rest[i + 5..];
        let trimmed = rest.trim_start();
        let end = trimmed.find(|c: char| !c.is_ascii_digit() && c != '-')
                         .unwrap_or(trimmed.len());
        if let Ok(n) = trimmed[..end].parse::<i64>() {
            if n > 0 && !out.contains(&n) {
                out.push(n);
            }
        }
        rest = &trimmed[end..];
    }
    out
}

fn build_window_title(parent: &GtkBox) {
    let lbl = Label::new(Some(""));
    lbl.set_max_width_chars(60);
    lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
    lbl.add_css_class("wilbar-window");
    parent.append(&lbl);

    let lbl_cl = lbl.clone();
    watch_hypr_events(move |evt, data| {
        if evt == "activewindow" {
            // data = "<class>,<title>"
            let title = data.splitn(2, ',').nth(1).unwrap_or("");
            lbl_cl.set_label(title);
        }
    });
}

fn build_audio(parent: &GtkBox) {
    let lbl = Label::new(Some(""));
    lbl.add_css_class("wilbar-status");
    parent.append(&lbl);
    let lbl_cl = lbl.clone();
    let tick = move || {
        let out = Command::new("wpctl")
            .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
            .output();
        let text = match out {
            Ok(o) if o.status.success() => {
                let s = String::from_utf8_lossy(&o.stdout);
                if s.contains("MUTED") {
                    "婢 muted".to_string()
                } else {
                    let frac = s.split_whitespace().nth(1).unwrap_or("0");
                    let pct = (frac.parse::<f64>().unwrap_or(0.0) * 100.0) as i32;
                    format!("  {pct}%")
                }
            }
            _ => "  ?".into(),
        };
        lbl_cl.set_label(&text);
    };
    tick();
    let lbl_t = lbl.clone();
    glib::timeout_add_local(Duration::from_secs(2), move || {
        let out = Command::new("wpctl")
            .args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
            .output();
        let text = match out {
            Ok(o) if o.status.success() => {
                let s = String::from_utf8_lossy(&o.stdout);
                if s.contains("MUTED") {
                    "婢 muted".to_string()
                } else {
                    let frac = s.split_whitespace().nth(1).unwrap_or("0");
                    let pct = (frac.parse::<f64>().unwrap_or(0.0) * 100.0) as i32;
                    format!("  {pct}%")
                }
            }
            _ => "  ?".into(),
        };
        lbl_t.set_label(&text);
        glib::ControlFlow::Continue
    });
}

fn build_battery(parent: &GtkBox) {
    let bat = first_battery();
    if bat.is_none() {
        return;
    }
    let lbl = Label::new(Some(""));
    lbl.add_css_class("wilbar-status");
    parent.append(&lbl);

    let lbl_t = lbl.clone();
    let bat_path = bat.unwrap();
    let tick = move || {
        let cap = read_int(&bat_path.join("capacity")).unwrap_or(0);
        let status = read_string(&bat_path.join("status")).unwrap_or_default();
        let glyph = if status == "Charging" { "" } else if cap > 80 { "" }
            else if cap > 50 { "" } else if cap > 20 { "" } else { "" };
        lbl_t.set_label(&format!("{glyph}  {cap}%"));
        if cap < 15 && status != "Charging" {
            lbl_t.add_css_class("wilbar-status-crit");
        } else if cap < 30 && status != "Charging" {
            lbl_t.add_css_class("wilbar-status-warn");
            lbl_t.remove_css_class("wilbar-status-crit");
        } else {
            lbl_t.remove_css_class("wilbar-status-warn");
            lbl_t.remove_css_class("wilbar-status-crit");
        }
    };
    tick();
    glib::timeout_add_local(Duration::from_secs(15), move || {
        tick();
        glib::ControlFlow::Continue
    });
}

fn first_battery() -> Option<PathBuf> {
    let dir = fs::read_dir("/sys/class/power_supply").ok()?;
    for ent in dir.flatten() {
        let path = ent.path();
        let name = path.file_name()?.to_string_lossy().to_string();
        if name.starts_with("BAT") {
            return Some(path);
        }
    }
    None
}

fn first_backlight() -> Option<PathBuf> {
    let dir = fs::read_dir("/sys/class/backlight").ok()?;
    dir.flatten().next().map(|e| e.path())
}

fn build_brightness(parent: &GtkBox) {
    let bl = first_backlight();
    if bl.is_none() {
        return;
    }
    let lbl = Label::new(Some(""));
    lbl.add_css_class("wilbar-status");
    parent.append(&lbl);

    let bl = bl.unwrap();
    let lbl_t = lbl.clone();
    let tick = move || {
        let cur = read_int(&bl.join("brightness")).unwrap_or(0) as f64;
        let max = read_int(&bl.join("max_brightness")).unwrap_or(1).max(1) as f64;
        let pct = ((cur / max) * 100.0) as i32;
        lbl_t.set_label(&format!("  {pct}%"));
    };
    tick();
    glib::timeout_add_local(Duration::from_secs(10), move || {
        tick();
        glib::ControlFlow::Continue
    });
}

fn build_wilai_mode(parent: &GtkBox) {
    let lbl = Label::new(Some(""));
    lbl.add_css_class("wilbar-wilai");
    parent.append(&lbl);

    let lbl_t = lbl.clone();
    let tick = move || {
        // Poll the daemon over its socket via `wilai mode show --json`.
        // Falls back to "down" if the daemon is not running, which is a
        // valid steady state on machines where wilai is not enabled.
        let out = Command::new("wilai")
            .args(["mode", "show", "--json"])
            .output();
        let (mode, in_flight, ok) = match out {
            Ok(o) if o.status.success() => {
                let s = String::from_utf8_lossy(&o.stdout);
                parse_wilai_json(&s)
            }
            _ => ("down".to_string(), 0u64, false),
        };
        let glyph = match mode.as_str() {
            "pentest" => "",
            "normal" => "",
            _ => "",
        };
        let body = if in_flight > 0 {
            format!("{glyph} {mode} ({in_flight})")
        } else {
            format!("{glyph} {mode}")
        };
        lbl_t.set_label(&body);
        lbl_t.remove_css_class("wilbar-wilai-pentest");
        lbl_t.remove_css_class("wilbar-wilai-down");
        if !ok {
            lbl_t.add_css_class("wilbar-wilai-down");
        } else if mode == "pentest" {
            lbl_t.add_css_class("wilbar-wilai-pentest");
        }
    };
    tick();
    glib::timeout_add_local(Duration::from_secs(3), move || {
        tick();
        glib::ControlFlow::Continue
    });
}

fn parse_wilai_json(s: &str) -> (String, u64, bool) {
    // Very small parser since wilbar avoids serde_json.
    // Accepts {"mode":"<m>","pentest_in_flight":<n>} plus a "down" form
    // that includes a "reason" field. Anything malformed is treated as down.
    let mode = extract_string(s, "\"mode\":");
    let in_flight = extract_int_field(s, "\"pentest_in_flight\":").unwrap_or(0).max(0) as u64;
    let ok = matches!(mode.as_deref(), Some("normal" | "pentest"));
    (mode.unwrap_or_else(|| "down".to_string()), in_flight, ok)
}

fn extract_string(json: &str, key: &str) -> Option<String> {
    let i = json.find(key)?;
    let rest = &json[i + key.len()..];
    let q1 = rest.find('"')?;
    let after = &rest[q1 + 1..];
    let q2 = after.find('"')?;
    Some(after[..q2].to_string())
}

fn build_network(parent: &GtkBox) {
    let lbl = Label::new(Some(""));
    lbl.add_css_class("wilbar-status");
    parent.append(&lbl);

    let lbl_t = lbl.clone();
    let tick = move || {
        let out = Command::new("nmcli")
            .args(["-t", "-f", "TYPE,STATE,CONNECTION", "device"])
            .output();
        let text = match out {
            Ok(o) if o.status.success() => {
                let s = String::from_utf8_lossy(&o.stdout);
                let mut summary = "睊".to_string();
                for line in s.lines() {
                    let parts: Vec<&str> = line.splitn(3, ':').collect();
                    if parts.len() != 3 || parts[1] != "connected" {
                        continue;
                    }
                    summary = match parts[0] {
                        "wifi"     => format!("  {}", parts[2]),
                        "ethernet" => format!("  {}", parts[2]),
                        _ => continue,
                    };
                    break;
                }
                summary
            }
            _ => "睊".into(),
        };
        lbl_t.set_label(&text);
    };
    tick();
    glib::timeout_add_local(Duration::from_secs(5), move || {
        tick();
        glib::ControlFlow::Continue
    });
}

// ---------- helpers ------------------------------------------------------- //

fn read_string(path: &std::path::Path) -> Option<String> {
    let mut s = String::new();
    fs::File::open(path).ok()?.read_to_string(&mut s).ok()?;
    Some(s.trim().to_string())
}

fn read_int(path: &std::path::Path) -> Option<i64> {
    read_string(path)?.parse().ok()
}

// ---------- main --------------------------------------------------------- //

fn build_ui(app: &Application) {
    let window = ApplicationWindow::builder()
        .application(app)
        .default_height(30)
        .build();
    window.set_widget_name("wilbar");

    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.auto_exclusive_zone_enable();
    for edge in [Edge::Top, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
        window.set_margin(edge, 6);
    }

    let root = GtkBox::new(Orientation::Horizontal, 8);
    root.add_css_class("wilbar-pad");

    let left = GtkBox::new(Orientation::Horizontal, 6);
    left.set_halign(Align::Start);
    left.set_hexpand(true);

    let center = GtkBox::new(Orientation::Horizontal, 8);
    center.set_halign(Align::Center);
    center.set_hexpand(true);

    let right = GtkBox::new(Orientation::Horizontal, 6);
    right.set_halign(Align::End);
    right.set_hexpand(true);

    let logo = Label::new(Some(""));
    logo.add_css_class("wilbar-pad");
    left.append(&logo);
    build_workspaces(&left);
    build_window_title(&left);

    build_clock(&center);

    build_wilai_mode(&right);
    build_audio(&right);
    build_brightness(&right);
    build_battery(&right);
    build_network(&right);

    root.append(&left);
    root.append(&center);
    root.append(&right);

    window.set_child(Some(&root));
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
