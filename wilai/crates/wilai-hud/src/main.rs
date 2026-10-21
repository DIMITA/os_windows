//! wilai-hud — permanent HUD overlay. Draws the Aurora robot face as a
//! Wayland layer-shell panel pinned above every window on Hyprland (or
//! any wlroots compositor). Pure-Rust: SCTK for Wayland, tiny-skia for
//! 2D, png for image decode. No GTK.
//!
//! The HUD subscribes to the daemon's event stream and reflects what
//! Wilai is doing (idle, thinking, speaking, running a tool, pentest
//! mode) by swapping the face state. Animation runs at ~30 FPS driven
//! by Wayland frame callbacks.

mod daemon_client;
mod face;

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

use daemon_client::SharedFaceState;
use face::FaceState;

#[derive(Parser, Debug)]
#[command(name = "wilai-hud", version, about = "Wilai permanent HUD overlay (Wayland layer-shell).")]
struct Cli {
    /// Daemon socket to subscribe to.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// HUD width in pixels.
    #[arg(long, default_value_t = 240)]
    width: u32,
    /// HUD height in pixels.
    #[arg(long, default_value_t = 260)]
    height: u32,
    /// Anchor edge: top-right, top-left, bottom-right, bottom-left,
    /// top, bottom.
    #[arg(long, default_value = "top-right")]
    anchor: String,
    /// Margin from the anchored edges, in pixels.
    #[arg(long, default_value_t = 12)]
    margin: i32,
    /// Path to the face PNG asset. Defaults to
    /// $XDG_DATA_HOME/wilai/assets/wilai-face.png, falling back to
    /// /usr/share/wilai/assets/wilai-face.png.
    #[arg(long)]
    face: Option<PathBuf>,
    /// Logging filter.
    #[arg(long, default_value = "warn")]
    log: String,
}

fn default_socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(p).join("wilai.sock");
    }
    PathBuf::from("/run/wilai.sock")
}

fn resolve_face_path(cli: &Cli) -> Option<PathBuf> {
    if let Some(p) = &cli.face {
        return Some(p.clone());
    }
    if let Some(d) = dirs::data_dir() {
        let p = d.join("wilai/assets/wilai-face.png");
        if p.exists() {
            return Some(p);
        }
    }
    let fallback = PathBuf::from("/usr/share/wilai/assets/wilai-face.png");
    if fallback.exists() {
        Some(fallback)
    } else {
        None
    }
}

fn init_logging(filter: &str) {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter)),
        )
        .with_target(false)
        .try_init();
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(&cli.log);

    let face_bytes = resolve_face_path(&cli).and_then(|p| std::fs::read(&p).ok());
    if face_bytes.is_none() {
        tracing::warn!(
            "no face image found at {:?} or defaults; falling back to schematic head",
            cli.face
        );
    }

    // Shared face state the subscriber task updates and the render loop
    // reads on every frame.
    let state = Arc::new(SharedFaceState::new(FaceState::Idle));
    let socket = cli.socket.clone().unwrap_or_else(default_socket_path);
    let state_sub = Arc::clone(&state);
    tokio::spawn(async move {
        let _ = daemon_client::run_subscriber(&socket, state_sub).await;
    });

    // Hand the render loop to a blocking thread so Wayland dispatch has
    // its own OS thread. tokio-aware tasks (the daemon subscriber) run
    // on the executor; the compositor loop is pure sync.
    let face_bytes = face_bytes.unwrap_or_default();
    let has_face = !face_bytes.is_empty();
    let state_render = Arc::clone(&state);
    let w = cli.width;
    let h = cli.height;
    let anchor = cli.anchor.clone();
    let margin = cli.margin;

    tokio::task::spawn_blocking(move || {
        let face = if has_face { Some(face_bytes.as_slice()) } else { None };
        if let Err(e) = wayland::run_hud(w, h, &anchor, margin, face, state_render) {
            tracing::error!("hud exited: {e:#}");
        }
    })
    .await
    .context("render thread join")?;

    Ok(())
}

/// Wayland driver — isolated in its own module so face/daemon_client
/// don't need to see SCTK types.
mod wayland {
    use crate::daemon_client::SharedFaceState;
    use crate::face::Renderer;
    use anyhow::{anyhow, Context, Result};
    use smithay_client_toolkit::{
        compositor::{CompositorHandler, CompositorState},
        delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
        output::{OutputHandler, OutputState},
        registry::{ProvidesRegistryState, RegistryState},
        registry_handlers,
        shell::{
            wlr_layer::{Anchor, Layer, LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure},
            WaylandSurface,
        },
        shm::{slot::SlotPool, Shm, ShmHandler},
    };
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tiny_skia::Pixmap;
    use wayland_client::{
        globals::registry_queue_init,
        protocol::{wl_output, wl_shm, wl_surface},
        Connection, QueueHandle,
    };

    pub fn run_hud(
        width: u32,
        height: u32,
        anchor_name: &str,
        margin: i32,
        face: Option<&[u8]>,
        state: Arc<SharedFaceState>,
    ) -> Result<()> {
        let conn = Connection::connect_to_env()
            .context("connect to the Wayland display (WAYLAND_DISPLAY unset?)")?;
        let (globals, mut event_queue) =
            registry_queue_init(&conn).context("registry handshake")?;
        let qh = event_queue.handle();

        let compositor = CompositorState::bind(&globals, &qh).context("wl_compositor")?;
        let layer_shell = LayerShell::bind(&globals, &qh).context("zwlr_layer_shell_v1")?;
        let shm = Shm::bind(&globals, &qh).context("wl_shm")?;

        let surface = compositor.create_surface(&qh);
        let layer = layer_shell.create_layer_surface(
            &qh,
            surface,
            Layer::Overlay,
            Some("wilai-hud"),
            None,
        );
        layer.set_size(width, height);
        layer.set_anchor(parse_anchor(anchor_name));
        layer.set_margin(margin, margin, margin, margin);
        layer.set_keyboard_interactivity(
            smithay_client_toolkit::shell::wlr_layer::KeyboardInteractivity::None,
        );
        layer.commit();

        let pool = SlotPool::new((width * height * 4) as usize, &shm).context("shm pool")?;
        let renderer = Renderer::new(face);

        let mut app = AppState {
            registry: RegistryState::new(&globals),
            output: OutputState::new(&globals, &qh),
            compositor,
            shm,
            pool,
            layer,
            renderer,
            face_state: state,
            width,
            height,
            configured: false,
            quit: false,
            tick: 0,
            start: Instant::now(),
        };

        // Draw at ~30 FPS, driven by frame callbacks and a sleep for
        // the first frame.
        while !app.quit {
            event_queue.blocking_dispatch(&mut app).context("wayland dispatch")?;
            if app.configured {
                app.render(&qh);
            }
            // Cap at ~30 FPS even if the compositor would gladly go faster.
            std::thread::sleep(Duration::from_millis(33));
        }
        Ok(())
    }

    fn parse_anchor(s: &str) -> Anchor {
        match s {
            "top-left"     => Anchor::TOP    | Anchor::LEFT,
            "top-right"    => Anchor::TOP    | Anchor::RIGHT,
            "bottom-left"  => Anchor::BOTTOM | Anchor::LEFT,
            "bottom-right" => Anchor::BOTTOM | Anchor::RIGHT,
            "top"          => Anchor::TOP,
            "bottom"       => Anchor::BOTTOM,
            "left"         => Anchor::LEFT,
            "right"        => Anchor::RIGHT,
            _              => Anchor::TOP | Anchor::RIGHT,
        }
    }

    struct AppState {
        registry: RegistryState,
        output: OutputState,
        #[allow(dead_code)]
        compositor: CompositorState,
        shm: Shm,
        pool: SlotPool,
        layer: LayerSurface,
        renderer: Renderer,
        face_state: Arc<SharedFaceState>,
        width: u32,
        height: u32,
        configured: bool,
        quit: bool,
        tick: u64,
        start: Instant,
    }

    impl AppState {
        fn render(&mut self, _qh: &QueueHandle<Self>) {
            let state = self.face_state.get();
            // tick phase derived from elapsed time so animation stays smooth
            // across frame-rate hiccups.
            self.tick = (self.start.elapsed().as_millis() as u64) / 33;

            let (buffer, canvas) = match self.pool.create_buffer(
                self.width as i32,
                self.height as i32,
                (self.width * 4) as i32,
                wl_shm::Format::Argb8888,
            ) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("shm buffer alloc failed: {e:#}");
                    return;
                }
            };

            // Paint via tiny-skia (RGBA) then convert to ARGB for Wayland.
            let mut pm = match Pixmap::new(self.width, self.height) {
                Some(p) => p,
                None => return,
            };
            self.renderer.render(&mut pm, state, self.tick);
            rgba_to_argb(pm.data(), canvas);

            let surface = self.layer.wl_surface();
            if buffer.attach_to(surface).is_err() {
                tracing::warn!("buffer attach failed");
                return;
            }
            surface.damage_buffer(0, 0, self.width as i32, self.height as i32);
            surface.commit();
        }
    }

    fn rgba_to_argb(src: &[u8], dst: &mut [u8]) {
        for (s, d) in src.chunks_exact(4).zip(dst.chunks_exact_mut(4)) {
            // tiny-skia pre-multiplied RGBA → Wayland ARGB8888 (little-endian: B, G, R, A).
            d[0] = s[2]; // B
            d[1] = s[1]; // G
            d[2] = s[0]; // R
            d[3] = s[3]; // A
        }
    }

    impl CompositorHandler for AppState {
        fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
        fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
        fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
        fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
        fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    }

    impl OutputHandler for AppState {
        fn output_state(&mut self) -> &mut OutputState {
            &mut self.output
        }
        fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
        fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
        fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    }

    impl LayerShellHandler for AppState {
        fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
            self.quit = true;
        }
        fn configure(
            &mut self,
            _: &Connection,
            _qh: &QueueHandle<Self>,
            _layer: &LayerSurface,
            configure: LayerSurfaceConfigure,
            _serial: u32,
        ) {
            if configure.new_size.0 > 0 {
                self.width = configure.new_size.0;
            }
            if configure.new_size.1 > 0 {
                self.height = configure.new_size.1;
            }
            self.configured = true;
        }
    }

    impl ShmHandler for AppState {
        fn shm_state(&mut self) -> &mut Shm {
            &mut self.shm
        }
    }

    impl ProvidesRegistryState for AppState {
        fn registry(&mut self) -> &mut RegistryState {
            &mut self.registry
        }
        registry_handlers![OutputState];
    }

    delegate_compositor!(AppState);
    delegate_output!(AppState);
    delegate_layer!(AppState);
    delegate_shm!(AppState);
    delegate_registry!(AppState);

    // Silence unused-import warnings when optional handlers aren't wired.
    #[allow(dead_code)]
    fn _compat_assert() {
        let _ = anyhow!("");
    }
}
