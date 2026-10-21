//! `wilai-shell` — full-screen TUI frontend.
//!
//! Boots to a Jarvis-like face + chat area + input field, connects to
//! wilai-daemon over its unix socket, streams server events into the
//! chat log, and sends ClientOp::Prompt back when the user hits Enter.
//! On a WilOS-core system (no desktop) this is the primary interface,
//! started by a systemd unit at tty1 autologin.

mod app;
mod client;
mod face;
mod ui;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;
use wilai_daemon::protocol::ServerEvent;

use app::{App, PendingConfirm, Who};
use client::DaemonClient;
use face::FaceState;

#[derive(Parser, Debug)]
#[command(name = "wilai-shell", version, about = "Wilai TUI shell — Wilai as the primary interface.")]
struct Cli {
    /// Daemon socket path. Defaults to $XDG_RUNTIME_DIR/wilai.sock.
    #[arg(long)]
    socket: Option<PathBuf>,
    /// Model override forwarded on each prompt.
    #[arg(long)]
    model: Option<String>,
    /// Provider override forwarded on each prompt.
    #[arg(long)]
    provider: Option<String>,
    /// Run the first-boot setup wizard then exit.
    #[arg(long)]
    wizard: bool,
    /// Logging filter (writes to $XDG_STATE_HOME/wilai/shell.log).
    #[arg(long, default_value = "warn")]
    log: String,
}

fn default_socket_path() -> PathBuf {
    if let Ok(p) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(p).join("wilai.sock");
    }
    PathBuf::from("/run/wilai.sock")
}

fn init_logging(filter: &str) {
    // The shell owns the terminal; we must not log to stderr. Send to
    // a file instead. Best-effort: silent fallback if we cannot open.
    let Some(mut state) = dirs::state_dir() else { return };
    state.push("wilai");
    let _ = std::fs::create_dir_all(&state);
    state.push("shell.log");
    let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&state) else {
        return;
    };
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(filter)))
        .with_writer(file)
        .with_ansi(false)
        .try_init();
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(&cli.log);

    let socket = cli.socket.clone().unwrap_or_else(default_socket_path);

    // Wizard-pending marker: either --wizard was passed, or the file
    // /var/lib/wilai/wizard-pending exists (laid down by the ISO
    // post-install hook). In both cases we seed the first turn with the
    // setup script so Wilai drives the user through locale/net/user.
    let wizard_seed = if cli.wizard || wizard_pending() {
        Some(WIZARD_SEED.to_string())
    } else {
        None
    };

    let mut term = setup_terminal()?;
    let res = run_app(
        &mut term,
        &socket,
        cli.model.clone(),
        cli.provider.clone(),
        wizard_seed,
    )
    .await;
    restore_terminal(&mut term)?;
    res
}

fn wizard_marker_path() -> PathBuf {
    PathBuf::from("/var/lib/wilai/wizard-pending")
}

fn wizard_pending() -> bool {
    wizard_marker_path().exists()
}

fn clear_wizard_marker() {
    let _ = std::fs::remove_file(wizard_marker_path());
}

const WIZARD_SEED: &str = "\
[FIRST-BOOT WIZARD]
This is a brand-new WilOS-Core installation. Walk the user through \
initial setup, speaking in the configured persona style. For each step, \
run the appropriate sys.* tool, then confirm with the user before moving \
on. Steps in order:

  1. Greet. Ask for the preferred language. Use sys.locale.list to show \
     a short list, then sys.locale.set once chosen.
  2. Network: run sys.net.wifi_scan. Ask which SSID to join, then \
     sys.net.wifi_connect.
  3. Hostname: ask for a machine name, then sys.hostname.set.
  4. User: ask for a login name and whether they want sudo, then \
     sys.user.add with wheel accordingly.
  5. Optional packages: ask what they typically use (dev, media, \
     office, security) and offer bundles via sys.pkg.install.
  6. Finish: tell the user setup is done and that from now on they can \
     just talk to you. Call memory.fact_set with key=\"setup\" value=\"done\".

Keep each step short. Do not run the next step until the previous one \
succeeded.
";

type Term = Terminal<CrosstermBackend<Stdout>>;

fn setup_terminal() -> Result<Term> {
    enable_raw_mode().context("enable raw mode")?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableMouseCapture).context("enter alt screen")?;
    let backend = CrosstermBackend::new(out);
    Terminal::new(backend).context("construct terminal")
}

fn restore_terminal(term: &mut Term) -> Result<()> {
    disable_raw_mode().ok();
    execute!(term.backend_mut(), LeaveAlternateScreen, DisableMouseCapture).ok();
    term.show_cursor().ok();
    Ok(())
}

async fn run_app(
    term: &mut Term,
    socket: &std::path::Path,
    model: Option<String>,
    provider: Option<String>,
    wizard_seed: Option<String>,
) -> Result<()> {
    let app = Arc::new(Mutex::new(App::new()));
    {
        let mut a = app.lock().await;
        a.push(Who::System, format!("connecting to {}...", socket.display()));
        if wizard_seed.is_some() {
            a.push(Who::System, "first-boot wizard will run on connect.");
        }
    }

    // Try to connect; if daemon is not up, keep showing the message and
    // retry a few times.
    let client = loop_connect(socket, Arc::clone(&app)).await?;
    let client = Arc::new(Mutex::new(client));

    // Auto-send the wizard seed as the first prompt.
    if let Some(seed) = wizard_seed {
        let mut c = client.lock().await;
        if let Err(e) = c.send_prompt(seed, model.clone(), provider.clone()).await {
            let mut a = app.lock().await;
            a.push(Who::System, format!("wizard seed failed: {e:#}"));
        } else {
            let mut a = app.lock().await;
            a.push(Who::System, "wizard seeded.");
            a.set_face(FaceState::Thinking);
            a.busy = true;
        }
        drop(c);
        clear_wizard_marker();
    }

    // Spawn the daemon-reader task.
    let (ev_tx, mut ev_rx) = tokio::sync::mpsc::channel::<ServerEvent>(64);
    let client_r = Arc::clone(&client);
    let reader_task = tokio::spawn(async move {
        loop {
            let next = {
                let mut c = client_r.lock().await;
                c.next_event().await
            };
            match next {
                Ok(ev) => {
                    if ev_tx.send(ev).await.is_err() {
                        return;
                    }
                }
                Err(_) => {
                    let _ = ev_tx.send(ServerEvent::Bye).await;
                    return;
                }
            }
        }
    });

    // Spawn a terminal-events reader.
    let (key_tx, mut key_rx) = tokio::sync::mpsc::channel::<KeyEvent>(32);
    let key_task = tokio::spawn(async move {
        loop {
            // crossterm's blocking poll is OK inside spawn_blocking; here
            // we use the thread approach via tokio::task::spawn_blocking.
            let got = tokio::task::spawn_blocking(|| {
                if crossterm::event::poll(Duration::from_millis(50)).unwrap_or(false) {
                    match crossterm::event::read() {
                        Ok(crossterm::event::Event::Key(k)) if k.kind == KeyEventKind::Press => Some(k),
                        _ => None,
                    }
                } else {
                    None
                }
            })
            .await;
            match got {
                Ok(Some(k)) => {
                    if key_tx.send(k).await.is_err() {
                        return;
                    }
                }
                Ok(None) => {}
                Err(_) => return,
            }
        }
    });

    // Draw loop.
    let mut tick_timer = tokio::time::interval(Duration::from_millis(100));
    loop {
        {
            let a = app.lock().await;
            term.draw(|f| ui::draw(f, &a))?;
            if a.quit {
                break;
            }
        }

        tokio::select! {
            _ = tick_timer.tick() => {
                let mut a = app.lock().await;
                a.tick_once();
            }
            Some(ev) = ev_rx.recv() => {
                let mut a = app.lock().await;
                handle_server_event(&mut a, ev);
            }
            Some(k) = key_rx.recv() => {
                let mut a = app.lock().await;
                if let Some(action) = handle_key(&mut a, k) {
                    drop(a);
                    match action {
                        KeyAction::SendPrompt(text) => {
                            let mut c = client.lock().await;
                            if let Err(e) = c.send_prompt(text, model.clone(), provider.clone()).await {
                                let mut a = app.lock().await;
                                a.push(Who::System, format!("send failed: {e:#}"));
                            }
                        }
                        KeyAction::Confirm { id, yes } => {
                            let mut c = client.lock().await;
                            let _ = c.send_confirm(id, yes).await;
                        }
                        KeyAction::Quit => {
                            let mut c = client.lock().await;
                            let _ = c.send_quit().await;
                            let mut a = app.lock().await;
                            a.quit = true;
                        }
                    }
                }
            }
        }
    }

    reader_task.abort();
    key_task.abort();
    Ok(())
}

async fn loop_connect(socket: &std::path::Path, app: Arc<Mutex<App>>) -> Result<DaemonClient> {
    for attempt in 0..40 {
        match DaemonClient::connect(socket).await {
            Ok(mut c) => match c.next_event().await? {
                ServerEvent::SessionStart { session } => {
                    let mut a = app.lock().await;
                    a.session = Some(session.clone());
                    a.status_right = "connected".into();
                    a.push(Who::System, format!("session {session} started."));
                    return Ok(c);
                }
                other => {
                    let mut a = app.lock().await;
                    a.push(Who::System, format!("unexpected handshake: {other:?}"));
                    return Err(anyhow::anyhow!("bad handshake"));
                }
            },
            Err(e) => {
                let mut a = app.lock().await;
                a.status_right = format!("retry {}/40", attempt + 1);
                if attempt == 0 {
                    a.push(Who::System, format!("daemon not reachable: {e:#}"));
                }
                drop(a);
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }
    Err(anyhow::anyhow!("daemon unreachable after 10s"))
}

enum KeyAction {
    SendPrompt(String),
    Confirm { id: String, yes: bool },
    Quit,
}

fn handle_key(app: &mut App, k: KeyEvent) -> Option<KeyAction> {
    // Confirm modal takes priority.
    if let Some(c) = app.pending_confirm.clone() {
        return match k.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                app.pending_confirm = None;
                app.push(Who::System, "confirm → yes");
                Some(KeyAction::Confirm { id: c.id, yes: true })
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                app.pending_confirm = None;
                app.push(Who::System, "confirm → no");
                Some(KeyAction::Confirm { id: c.id, yes: false })
            }
            KeyCode::Enter => {
                let yes = c.default_yes;
                app.pending_confirm = None;
                app.push(Who::System, format!("confirm → {} (default)", if yes { "yes" } else { "no" }));
                Some(KeyAction::Confirm { id: c.id, yes })
            }
            _ => None,
        };
    }

    match (k.code, k.modifiers) {
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => Some(KeyAction::Quit),
        (KeyCode::Char('d'), KeyModifiers::CONTROL) if app.input.is_empty() => Some(KeyAction::Quit),
        (KeyCode::Enter, _) => {
            let text = app.input.trim().to_string();
            if text.is_empty() {
                return None;
            }
            app.input.clear();
            app.push(Who::You, text.clone());
            app.set_face(FaceState::Thinking);
            app.busy = true;
            Some(KeyAction::SendPrompt(text))
        }
        (KeyCode::Backspace, _) => {
            app.input.pop();
            None
        }
        (KeyCode::Char(ch), _) => {
            app.input.push(ch);
            None
        }
        (KeyCode::PageUp, _) => {
            app.scroll = app.scroll.saturating_sub(5);
            None
        }
        (KeyCode::PageDown, _) => {
            app.scroll = app.scroll.saturating_add(5);
            None
        }
        _ => None,
    }
}

fn handle_server_event(app: &mut App, ev: ServerEvent) {
    match ev {
        ServerEvent::SessionStart { session } => {
            app.session = Some(session);
        }
        ServerEvent::Text { content } => {
            app.push(Who::Wilai, content);
            app.set_face(FaceState::Speaking);
        }
        ServerEvent::ToolCall { name, args } => {
            app.push(Who::Tool, format!("→ {name} {args}"));
            app.set_face(FaceState::Acting);
        }
        ServerEvent::ToolResult { name, ok, output } => {
            let mark = if ok { "✓" } else { "✗" };
            let snippet = if output.len() > 400 {
                format!("{}… (+{} bytes)", &output[..400], output.len() - 400)
            } else {
                output
            };
            app.push(Who::Tool, format!("{mark} {name}: {snippet}"));
        }
        ServerEvent::ConfirmAsk { id, prompt, default, timeout_s: _ } => {
            app.pending_confirm = Some(PendingConfirm {
                id,
                prompt,
                default_yes: default == "yes",
            });
        }
        ServerEvent::TurnDone => {
            app.busy = false;
            app.set_face(FaceState::Idle);
        }
        ServerEvent::Error { message } => {
            app.push(Who::System, format!("error: {message}"));
            app.busy = false;
            app.set_face(FaceState::Alert);
        }
        ServerEvent::Mode { current, pentest_in_flight: _ } => {
            app.mode = current;
        }
        ServerEvent::Tools { .. } => {}
        ServerEvent::ToolsAdded { names } => {
            app.push(Who::System, format!("new tools: {}", names.join(", ")));
        }
        ServerEvent::McpServers { .. } => {}
        ServerEvent::Bye => {
            app.push(Who::System, "daemon closed the session.");
            app.set_face(FaceState::Offline);
            app.quit = true;
        }
    }
}

