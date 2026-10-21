//! `wilai-shell` — full-screen TUI frontend.
//!
//! Boots to a Jarvis-like face + chat area + input field, connects to
//! wilai-daemon over its unix socket, streams server events into the
//! chat log, and sends ClientOp::Prompt back when the user hits Enter.
//! On a WilOS-core system (no desktop) this is the primary interface,
//! started by a systemd unit at tty1 autologin.
//!
//! v3.6 adds local voice I/O: push-to-talk on F2, optional wake-word
//! via an external command, STT via whisper.cpp, TTS via piper — all
//! run as subprocesses so this binary carries no audio dependencies.

mod app;
mod client;
mod face;
mod ui;
mod voice;

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::{mpsc, Mutex};
use tracing_subscriber::EnvFilter;
use wilai_core::VoiceConfig;
use wilai_daemon::protocol::ServerEvent;

use app::{App, PendingConfirm, Who};
use client::DaemonClient;
use face::FaceState;
use voice::VoiceEngine;

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
    /// Start in installer mode: seed the agent with the install script
    /// that walks the user through disk layout, pacstrap, bootloader.
    #[arg(long)]
    install: bool,
    /// Enable voice I/O (push-to-talk on F2, TTS of replies).
    #[arg(long)]
    voice: bool,
    /// Override VoiceConfig.whisper_model.
    #[arg(long)]
    whisper_model: Option<String>,
    /// Override VoiceConfig.piper_model.
    #[arg(long)]
    piper_model: Option<String>,
    /// Override VoiceConfig.record_secs.
    #[arg(long)]
    record_secs: Option<u32>,
    /// External wake-word command (reads stdout lines; each line triggers a record).
    #[arg(long)]
    wake_cmd: Option<String>,
    /// Logging filter (writes to $XDG_STATE_HOME/wilai/shell.log).
    #[arg(long, default_value = "warn")]
    log: String,
}

fn default_socket_path() -> PathBuf {
    // Try the per-user XDG socket first (user-local daemon), then the
    // system-wide socket paths, in order of likelihood. The .deb's
    // systemd unit puts the socket under RuntimeDirectory=wilai, so
    // /run/wilai/wilai.sock exists first; the WilOS-Core profile uses
    // the flat /run/wilai.sock. Pick the first that exists and is a
    // socket.
    if let Ok(p) = std::env::var("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(p).join("wilai.sock");
        if p.exists() { return p; }
    }
    for candidate in ["/run/wilai/wilai.sock", "/run/wilai.sock"] {
        let p = PathBuf::from(candidate);
        if p.exists() { return p; }
    }
    // None exist yet — return the deb path by default so the error
    // message points somewhere useful.
    PathBuf::from("/run/wilai/wilai.sock")
}

fn init_logging(filter: &str) {
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

fn resolve_voice_config(cli: &Cli) -> VoiceConfig {
    let mut cfg = match wilai_core::Config::load() {
        Ok(c) => c.voice,
        Err(_) => VoiceConfig::default(),
    };
    if cli.voice {
        cfg.enabled = true;
    }
    if let Some(m) = &cli.whisper_model {
        cfg.whisper_model = m.clone();
    }
    if let Some(m) = &cli.piper_model {
        cfg.piper_model = m.clone();
    }
    if let Some(s) = cli.record_secs {
        cfg.record_secs = s;
    }
    if let Some(c) = &cli.wake_cmd {
        cfg.wake_cmd = c.clone();
    }
    cfg
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(&cli.log);

    let socket = cli.socket.clone().unwrap_or_else(default_socket_path);
    let voice_cfg = resolve_voice_config(&cli);
    let voice_engine = if voice_cfg.enabled {
        Some(VoiceEngine::new(voice_cfg))
    } else {
        None
    };

    let wizard_seed = if cli.install || install_pending() {
        Some(INSTALL_SEED.to_string())
    } else if cli.wizard || wizard_pending() {
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
        voice_engine,
    )
    .await;
    restore_terminal(&mut term)?;
    res
}

fn wizard_marker_path() -> PathBuf {
    PathBuf::from("/var/lib/wilai/wizard-pending")
}

fn install_marker_path() -> PathBuf {
    PathBuf::from("/var/lib/wilai/install-pending")
}

fn wizard_pending() -> bool {
    wizard_marker_path().exists()
}

fn install_pending() -> bool {
    install_marker_path().exists()
}

fn clear_wizard_marker() {
    let _ = std::fs::remove_file(wizard_marker_path());
    let _ = std::fs::remove_file(install_marker_path());
}

/// Lay the install-pending marker so the next shell boot (or a /install
/// slash command inside the current session) re-seeds in installer mode.
fn set_install_marker() -> std::io::Result<()> {
    if let Some(parent) = install_marker_path().parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(install_marker_path(), b"1")
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
  7. Ask the user whether they want to install WilOS to a disk now. If \
     yes, tell them to type `/install`.

Keep each step short. Do not run the next step until the previous one \
succeeded.
";

const ROLLBACK_SEED: &str = "\
[ROLLBACK]
The user wants to roll Wilai back to an earlier version. Call \
sys.wilai.rollback_list to see what snapshots exist. Read the \
output back to the user: for each snapshot, show its version, \
mark the current one, and note that older ones are stored under \
/var/lib/wilai/rollback. Ask which version to restore. If the \
user picks one, call sys.wilai.rollback with that version. Warn \
the user that: 1) the daemon will restart and the shell will \
briefly show 'connecting…', 2) if the memory DB schema is newer \
than the target version expects, memory tools may fail until a \
re-upgrade. If no snapshots are listed, tell the user that \
/update takes a snapshot automatically — this becomes useful \
after the next upgrade.
";

const REMOTES_SEED: &str = "\
[REMOTES]
List the federated Wilai instances declared in [remotes]. For each \
one, show its name, description and SSH target. If none are declared, \
tell the user how to add one (see the [remotes] section in \
/usr/share/wilai/wilai.toml.example). Then offer to reach out to one \
with remote.wilai.read.
";

const UPDATE_SEED: &str = "\
[SELF-UPDATE]
The user wants to upgrade Wilai itself. Call sys.wilai.update first \
with dry_run=true to show whether a new version is available. If it \
is, tell the user the current and candidate versions and ask whether \
to proceed. On yes, call sys.wilai.update with dry_run=false. The \
shell will briefly disconnect while the daemon restarts and then \
reconnect automatically; mention that so the user does not panic.
";

const INSTALL_SEED: &str = "\
[INSTALLER]
The user wants to install WilOS to a disk. You are running from the \
live ISO; the installer tools are available under sys.disk.* and \
sys.install.*. Walk through the sequence below, confirming with the \
user at each decision point. Speak in the configured persona style; \
be explicit about what will be lost.

  1. INVENTORY. Run sys.disk.list and read the output back to the user. \
     Ask which block device to install onto (e.g. /dev/sda or \
     /dev/nvme0n1). If several candidates exist, list each with its \
     size + label.
  2. CONFIRM WIPE. Make the user acknowledge, in their own words, that \
     every byte on the chosen device will be destroyed. Then call \
     sys.disk.wipe with that device.
  3. PARTITION. Default scheme on EFI:
       - part 1: 512M, type ef00, label ESP
       - part 2: rest, type 8304, label root
     Call sys.disk.sgdisk_new twice with those arguments. If the user \
     wants swap, insert it as part 2 (type 8200, label swap, size \
     chosen by the user), then part 3 becomes root.
  4. FORMAT. sys.disk.format each partition: vfat on the ESP, ext4 (or \
     btrfs if the user asks) on root, swap on the swap partition. \
     Activate swap now with sys.disk.swapon.
  5. MOUNT. sys.install.target_mount for root at /mnt, then \
     sys.install.esp_mount for the ESP at /mnt/boot.
  6. PACSTRAP. sys.install.pacstrap /mnt with: base base-devel linux \
     linux-firmware intel-ucode amd-ucode networkmanager pipewire \
     pipewire-pulse wireplumber alsa-utils openssh grub efibootmgr \
     sudo nano vim git ollama wilai whisper.cpp piper.
  7. FSTAB. sys.install.genfstab /mnt.
  8. TIMEZONE + LOCALE. sys.install.chroot_run /mnt with [\"ln\", \"-sf\", \
     \"/usr/share/zoneinfo/<tz>\", \"/etc/localtime\"] using the tz the \
     user tells you. Then sys.install.chroot_run with [\"hwclock\", \
     \"--systohc\"]. Then sys.install.chroot_run with [\"locale-gen\"] \
     after writing /etc/locale.gen inside the chroot.
  9. HOSTNAME. Ask for a hostname, then sys.install.chroot_run with \
     [\"sh\", \"-c\", \"echo <name> > /etc/hostname\"].
  10. USER. sys.install.user_create for the primary user (wheel=true \
     unless they say otherwise). Then ask for a password and run \
     sys.install.chroot_run with [\"passwd\", \"<user>\"] — the shell \
     will relay the typed password.
  11. BOOTLOADER. sys.install.bootloader_grub target=/mnt esp=/mnt/boot.
  12. SERVICES. sys.install.enable_service for each of: \
     NetworkManager.service, systemd-timesyncd.service, ollama.service, \
     wilai-daemon.service, wilai-shell.service.
  13. CARRY STATE. sys.install.copy_wilai_state target=/mnt so the \
     facts and notes you learned on the live session survive the reboot.
  14. FINISH. Tell the user the installation is complete, that they \
     should reboot and remove the install medium, and that Wilai will \
     greet them again on the new system.

Rules:
  - NEVER run a destructive step without the user acknowledging it.
  - If a step fails, STOP, report the error, and ask the user how to \
    proceed. Do not retry blindly.
  - Keep each message short — one step at a time.
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

/// Events that bubble up from the voice tasks (record+transcribe,
/// wake-word watcher, TTS speak queue).
#[derive(Debug)]
enum VoiceEvent {
    WakeTriggered,
    ListenStart,
    Transcribed(String),
    ListenError(String),
    SpeakDone,
    SpeakError(String),
}

async fn run_app(
    term: &mut Term,
    socket: &std::path::Path,
    model: Option<String>,
    provider: Option<String>,
    wizard_seed: Option<String>,
    voice: Option<VoiceEngine>,
) -> Result<()> {
    let app = Arc::new(Mutex::new(App::new()));
    {
        let mut a = app.lock().await;
        a.push(Who::System, format!("connecting to {}...", socket.display()));
        if wizard_seed.is_some() {
            a.push(Who::System, "first-boot wizard will run on connect.");
        }
        if let Some(v) = &voice {
            let probe = v.probe();
            a.push(Who::System, format!("voice: {}", probe.summary()));
            a.voice_enabled = true;
            a.voice_can_listen = v.can_listen();
            a.voice_can_speak = v.can_speak();
            if !v.cfg.wake_cmd.is_empty() {
                a.push(Who::System, format!("wake-cmd: {}", v.cfg.wake_cmd));
            }
        }
    }

    let client = loop_connect(socket, Arc::clone(&app)).await?;
    let client = Arc::new(Mutex::new(client));

    // Auto-send wizard seed first.
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

    // Daemon reader task.
    let (ev_tx, mut ev_rx) = mpsc::channel::<ServerEvent>(64);
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

    // Terminal events task.
    let (key_tx, mut key_rx) = mpsc::channel::<KeyEvent>(32);
    let key_task = tokio::spawn(async move {
        loop {
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

    // Voice tasks: wake-word watcher + TTS speak queue.
    let (voice_tx, mut voice_rx) = mpsc::channel::<VoiceEvent>(32);
    let (tts_tx, _tts_task_handle, _wake_task_handle) = spawn_voice_tasks(voice.clone(), voice_tx.clone());

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
                let text_for_tts = if let ServerEvent::Text { content } = &ev {
                    Some(content.clone())
                } else {
                    None
                };
                let mut a = app.lock().await;
                handle_server_event(&mut a, ev);
                let muted = a.voice_mute;
                drop(a);
                if !muted {
                    if let (Some(text), Some(tx)) = (text_for_tts, &tts_tx) {
                        for phrase in VoiceEngine::split_phrases(&text) {
                            if phrase.trim().len() < 2 {
                                continue;
                            }
                            let _ = tx.send(TtsCommand::Say(phrase)).await;
                        }
                    }
                }
            }
            Some(k) = key_rx.recv() => {
                let mut a = app.lock().await;
                if let Some(action) = handle_key(&mut a, k) {
                    drop(a);
                    dispatch_action(action, &client, &app, &model, &provider, &voice, &voice_tx).await;
                }
            }
            Some(vev) = voice_rx.recv() => {
                handle_voice_event(vev, &app, &client, &model, &provider, &voice, &voice_tx).await;
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
    VoiceStart,
    VoiceToggleMute,
}

fn handle_key(app: &mut App, k: KeyEvent) -> Option<KeyAction> {
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
        (KeyCode::F(2), _) => {
            if app.voice_enabled && app.voice_can_listen && !app.recording && !app.busy {
                Some(KeyAction::VoiceStart)
            } else {
                None
            }
        }
        (KeyCode::F(3), _) => {
            if app.voice_enabled {
                Some(KeyAction::VoiceToggleMute)
            } else {
                None
            }
        }
        (KeyCode::Enter, _) => {
            let text = app.input.trim().to_string();
            if text.is_empty() {
                return None;
            }
            app.input.clear();
            // Local slash commands intercepted by the shell.
            if text == "/install" {
                app.push(Who::You, text.clone());
                app.push(Who::System, "entering installer mode…");
                if let Err(e) = set_install_marker() {
                    app.push(Who::System, format!("could not lay install marker: {e}"));
                }
                app.set_face(FaceState::Thinking);
                app.busy = true;
                return Some(KeyAction::SendPrompt(INSTALL_SEED.to_string()));
            }
            if text == "/wizard" {
                app.push(Who::You, text.clone());
                app.push(Who::System, "re-running the first-boot wizard…");
                app.set_face(FaceState::Thinking);
                app.busy = true;
                return Some(KeyAction::SendPrompt(WIZARD_SEED.to_string()));
            }
            if text == "/update" {
                app.push(Who::You, text.clone());
                app.push(Who::System, "checking for a Wilai update…");
                app.set_face(FaceState::Thinking);
                app.busy = true;
                return Some(KeyAction::SendPrompt(UPDATE_SEED.to_string()));
            }
            if text == "/rollback" {
                app.push(Who::You, text.clone());
                app.push(Who::System, "listing rollback snapshots…");
                app.set_face(FaceState::Thinking);
                app.busy = true;
                return Some(KeyAction::SendPrompt(ROLLBACK_SEED.to_string()));
            }
            if text == "/remotes" {
                app.push(Who::You, text.clone());
                app.push(Who::System, "listing federated Wilai instances…");
                app.set_face(FaceState::Thinking);
                app.busy = true;
                return Some(KeyAction::SendPrompt(REMOTES_SEED.to_string()));
            }
            if text == "/help" || text == "?" {
                app.remember(&text);
                app.push(Who::You, text.clone());
                for line in HELP_TEXT.lines() {
                    app.push(Who::System, line.to_string());
                }
                return None;
            }
            app.remember(&text);
            app.push(Who::You, text.clone());
            app.set_face(FaceState::Thinking);
            app.busy = true;
            Some(KeyAction::SendPrompt(text))
        }
        (KeyCode::Backspace, _) => {
            app.input.pop();
            None
        }
        (KeyCode::Up, _) => {
            app.history_prev();
            None
        }
        (KeyCode::Down, _) => {
            app.history_next();
            None
        }
        (KeyCode::Esc, _) if app.history_cursor.is_some() => {
            app.history_cursor = None;
            if let Some(saved) = app.input_before_history.take() {
                app.input = saved;
            } else {
                app.input.clear();
            }
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

const HELP_TEXT: &str = "\
Wilai shell — commands, keybinds, workflows
───────────────────────────────────────────
Slash commands (type at the prompt and hit Enter):
  /help, ?       show this card
  /wizard        re-run the first-boot setup (locale, wifi, user, …)
  /install       install WilOS from the live ISO to a disk
  /update        check for a Wilai update and apply it
  /rollback      list snapshots and restore an earlier Wilai version
  /remotes       list federated Wilai instances declared in [remotes]

Keybinds:
  Enter          send prompt
  Up / Down      recall previous / newer prompt from history
  Esc            cancel history recall / dismiss a confirm
  Backspace      delete char
  PgUp / PgDn    scroll the conversation
  F2             push-to-talk (voice I/O must be enabled)
  F3             toggle TTS mute
  Ctrl+C         quit
  Ctrl+D         quit (when the input is empty)

Natural-language workflows — just ask, Wilai picks the tool:
  System       'what services are running?', 'disk list', 'wifi scan',
               'install the firefox package', 'restart nginx'
  Memory       'remember that my DB host is db.lan', 'what do you know
               about X?', 'recall notes about ssh config'
  Vision       'take a screenshot and describe it'
               (desktop only, forbidden in pentest mode)
  Federation   'ask homelab what services are running'
               (needs a [remotes] entry; see /remotes)

Pentest project (requires 'enter pentest mode' first):
  'start a pentest project named acme-webapp as a webapp engagement'
  'pin the authz document at /path/to/engagement.pdf'
  'add scope allow for the domain *.acme-stage.example'
  'is 10.1.5.5 in scope?'
  'run nmap on acme-stage.example'            (scope-checked)
  'add a critical finding: pre-auth RCE in /upload, CVSS vector
   AV:N/AC:L/PR:N/UI:N/S:C/C:H/I:H/A:H'
  'move finding <id> to confirmed'
  'render the report'
  'close the project'

Ask 'what tools do you have?' to see the live tool list.\
";

async fn dispatch_action(
    action: KeyAction,
    client: &Arc<Mutex<DaemonClient>>,
    app: &Arc<Mutex<App>>,
    model: &Option<String>,
    provider: &Option<String>,
    voice: &Option<VoiceEngine>,
    voice_tx: &mpsc::Sender<VoiceEvent>,
) {
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
        KeyAction::VoiceStart => {
            if let Some(engine) = voice.clone() {
                let mut a = app.lock().await;
                a.recording = true;
                a.set_face(FaceState::Listening);
                a.push(Who::System, "listening…");
                drop(a);
                let tx = voice_tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(VoiceEvent::ListenStart).await;
                    match engine.record_and_transcribe().await {
                        Ok(text) => { let _ = tx.send(VoiceEvent::Transcribed(text)).await; }
                        Err(e) => { let _ = tx.send(VoiceEvent::ListenError(format!("{e:#}"))).await; }
                    }
                });
            }
        }
        KeyAction::VoiceToggleMute => {
            let mut a = app.lock().await;
            a.voice_mute = !a.voice_mute;
            let state = if a.voice_mute { "muted" } else { "unmuted" };
            a.push(Who::System, format!("tts {state}"));
        }
    }
}

async fn handle_voice_event(
    ev: VoiceEvent,
    app: &Arc<Mutex<App>>,
    client: &Arc<Mutex<DaemonClient>>,
    model: &Option<String>,
    provider: &Option<String>,
    voice: &Option<VoiceEngine>,
    voice_tx: &mpsc::Sender<VoiceEvent>,
) {
    match ev {
        VoiceEvent::WakeTriggered => {
            let (should_record, engine) = {
                let a = app.lock().await;
                let should = a.voice_enabled && a.voice_can_listen && !a.recording && !a.busy;
                (should, voice.clone())
            };
            if should_record {
                if let Some(engine) = engine {
                    let mut a = app.lock().await;
                    a.recording = true;
                    a.set_face(FaceState::Listening);
                    a.push(Who::System, "wake-word → listening…");
                    drop(a);
                    let tx = voice_tx.clone();
                    tokio::spawn(async move {
                        match engine.record_and_transcribe().await {
                            Ok(text) => { let _ = tx.send(VoiceEvent::Transcribed(text)).await; }
                            Err(e) => { let _ = tx.send(VoiceEvent::ListenError(format!("{e:#}"))).await; }
                        }
                    });
                }
            }
        }
        VoiceEvent::ListenStart => {}
        VoiceEvent::Transcribed(text) => {
            let text = text.trim().to_string();
            let mut a = app.lock().await;
            a.recording = false;
            if text.is_empty() {
                a.push(Who::System, "(no speech detected)");
                a.set_face(FaceState::Idle);
                return;
            }
            a.push(Who::You, format!("🎙 {text}"));
            a.set_face(FaceState::Thinking);
            a.busy = true;
            drop(a);
            let mut c = client.lock().await;
            if let Err(e) = c.send_prompt(text, model.clone(), provider.clone()).await {
                let mut a = app.lock().await;
                a.push(Who::System, format!("send failed: {e:#}"));
                a.busy = false;
                a.set_face(FaceState::Alert);
            }
        }
        VoiceEvent::ListenError(msg) => {
            let mut a = app.lock().await;
            a.recording = false;
            a.push(Who::System, format!("listen error: {msg}"));
            a.set_face(FaceState::Alert);
        }
        VoiceEvent::SpeakDone => {}
        VoiceEvent::SpeakError(msg) => {
            let mut a = app.lock().await;
            a.push(Who::System, format!("tts error: {msg}"));
        }
    }
}

#[derive(Debug)]
enum TtsCommand {
    Say(String),
}

/// Spawn the TTS speak queue + optional wake-word watcher. Returns the
/// TtsCommand sender and the two task join handles.
fn spawn_voice_tasks(
    voice: Option<VoiceEngine>,
    voice_tx: mpsc::Sender<VoiceEvent>,
) -> (
    Option<mpsc::Sender<TtsCommand>>,
    Option<tokio::task::JoinHandle<()>>,
    Option<tokio::task::JoinHandle<()>>,
) {
    let Some(engine) = voice else {
        return (None, None, None);
    };

    // TTS speak queue: serialized consumer so phrases play in order.
    let tts_tx = if engine.can_speak() {
        let (tx, mut rx) = mpsc::channel::<TtsCommand>(32);
        let engine_c = engine.clone();
        let evtx = voice_tx.clone();
        let h = tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    TtsCommand::Say(text) => {
                        if let Err(e) = engine_c.speak(&text).await {
                            let _ = evtx.send(VoiceEvent::SpeakError(format!("{e:#}"))).await;
                        } else {
                            let _ = evtx.send(VoiceEvent::SpeakDone).await;
                        }
                    }
                }
            }
        });
        drop(h);
        Some(tx)
    } else {
        None
    };

    // Optional wake-word watcher.
    let wake_task = if !engine.cfg.wake_cmd.is_empty() {
        let cmd = engine.cfg.wake_cmd.clone();
        let evtx = voice_tx.clone();
        let h = tokio::spawn(async move {
            let child = Command::new("sh")
                .arg("-c")
                .arg(&cmd)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            let Ok(mut child) = child else { return };
            let Some(stdout) = child.stdout.take() else { return };
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(_line)) = lines.next_line().await {
                if evtx.send(VoiceEvent::WakeTriggered).await.is_err() {
                    break;
                }
            }
            let _ = child.wait().await;
        });
        Some(h)
    } else {
        None
    };

    (tts_tx, None, wake_task)
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
        ServerEvent::Nudge { kind, severity, message, .. } => {
            let who = Who::Nudge;
            let prefix = match severity.as_str() {
                "alert" => "⚠",
                "warn"  => "!",
                _       => "·",
            };
            app.push(who, format!("{prefix} [{kind}] {message}"));
            app.set_face(match severity.as_str() {
                "alert" => FaceState::Alert,
                _ => app.face,
            });
        }
        ServerEvent::Bye => {
            app.push(Who::System, "daemon closed the session.");
            app.set_face(FaceState::Offline);
            app.quit = true;
        }
    }
}
