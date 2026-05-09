//! Wilbar — native Aurora top bar for WilOS.
//!
//! v0 scaffold: a glass slab anchored to the top edge with three
//! regions (left / center / right). The center shows a live clock.
//! Modules and config will land in v0.1; this crate exists today so
//! the work can compile, ship the systemd unit, and let later
//! sessions add modules incrementally.
//!
//! Until wilbar reaches feature parity with Waybar, the live ISO
//! still ships Waybar — wilbar is opt-in via $WILOS_USE_WILBAR=1.

use chrono::Local;
use gtk::glib;
use gtk::prelude::*;
use gtk::{
    Align, Application, ApplicationWindow, Box as GtkBox, CssProvider, Label,
    Orientation, gdk::Display, style_context_add_provider_for_display,
    STYLE_PROVIDER_PRIORITY_APPLICATION,
};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use std::time::Duration;

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
.wilbar-clock {
    color: #FFFFFF;
    font-weight: 600;
    padding: 0 14px;
    background: rgba(255, 255, 255, 0.04);
    border-radius: 10px;
}
.wilbar-pad {
    padding: 4px 12px;
}
"#;

fn main() -> glib::ExitCode {
    let app = Application::builder().application_id(APP_ID).build();
    app.connect_startup(|_| load_css());
    app.connect_activate(build_ui);
    app.run()
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

fn build_ui(app: &Application) {
    let window = ApplicationWindow::builder()
        .application(app)
        .default_height(30)
        .build();
    window.set_widget_name("wilbar");

    // Layer-shell anchoring: top, full width, push other windows down.
    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.auto_exclusive_zone_enable();
    for edge in [Edge::Top, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
        window.set_margin(edge, 6);
    }

    // Outer container with three sections (left / center / right).
    let root = GtkBox::new(Orientation::Horizontal, 8);
    root.add_css_class("wilbar-pad");

    let left = GtkBox::new(Orientation::Horizontal, 8);
    left.set_halign(Align::Start);
    left.set_hexpand(true);

    let center = GtkBox::new(Orientation::Horizontal, 8);
    center.set_halign(Align::Center);
    center.set_hexpand(true);

    let right = GtkBox::new(Orientation::Horizontal, 8);
    right.set_halign(Align::End);
    right.set_hexpand(true);

    // Left: WilOS logo trigger + workspace placeholder.
    let logo = Label::new(Some(""));
    logo.add_css_class("wilbar-pad");
    left.append(&logo);

    // Center: live clock, refreshed every second.
    let clock = Label::new(Some(""));
    clock.add_css_class("wilbar-clock");
    update_clock(&clock);
    let clock_clone = clock.clone();
    glib::timeout_add_local(Duration::from_secs(1), move || {
        update_clock(&clock_clone);
        glib::ControlFlow::Continue
    });
    center.append(&clock);

    // Right: placeholder text until the modules land.
    let placeholder = Label::new(Some("modules coming"));
    placeholder.add_css_class("wilbar-pad");
    right.append(&placeholder);

    root.append(&left);
    root.append(&center);
    root.append(&right);

    window.set_child(Some(&root));
    window.present();
}

fn update_clock(label: &Label) {
    label.set_label(&Local::now().format("%a %d %b   %H:%M").to_string());
}
