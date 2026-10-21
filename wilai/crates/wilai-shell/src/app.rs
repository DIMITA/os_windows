//! App state for the Wilai TUI shell. Owns the chat log, the input
//! buffer, the current face state, and the pending confirm.

use crate::face::FaceState;
use std::collections::VecDeque;

#[derive(Debug, Clone)]
pub struct ChatLine {
    pub who: Who,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    You,
    Wilai,
    Tool,
    System,
    Nudge,
}

impl Who {
    pub fn tag(&self) -> &'static str {
        match self {
            Who::You => "you  ",
            Who::Wilai => "wilai",
            Who::Tool => "tool ",
            Who::System => "sys  ",
            Who::Nudge => "nudge",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PendingConfirm {
    pub id: String,
    pub prompt: String,
    pub default_yes: bool,
}

pub struct App {
    pub log: VecDeque<ChatLine>,
    pub input: String,
    pub face: FaceState,
    pub tick: u64,
    pub session: Option<String>,
    pub mode: String,
    pub pending_confirm: Option<PendingConfirm>,
    pub status_right: String,
    pub busy: bool,
    pub quit: bool,
    pub max_log_lines: usize,
    pub scroll: u16,
    /// Voice I/O enabled at CLI/config time.
    pub voice_enabled: bool,
    /// STT engines + model are present on disk.
    pub voice_can_listen: bool,
    /// TTS engines + model are present on disk.
    pub voice_can_speak: bool,
    /// User-toggled mute (F3).
    pub voice_mute: bool,
    /// A record+transcribe round is in flight.
    pub recording: bool,
    /// Prompts the user has sent, newest last. Up/Down recall.
    pub history: VecDeque<String>,
    /// Index into history during recall. None = at the live input.
    pub history_cursor: Option<usize>,
    /// Saved input buffer while browsing history (restored on Esc).
    pub input_before_history: Option<String>,
}

impl App {
    pub fn new() -> Self {
        Self {
            log: VecDeque::new(),
            input: String::new(),
            face: FaceState::Idle,
            tick: 0,
            session: None,
            mode: "normal".into(),
            pending_confirm: None,
            status_right: "connecting…".into(),
            busy: false,
            quit: false,
            max_log_lines: 1000,
            scroll: 0,
            voice_enabled: false,
            voice_can_listen: false,
            voice_can_speak: false,
            voice_mute: false,
            recording: false,
            history: VecDeque::new(),
            history_cursor: None,
            input_before_history: None,
        }
    }

    /// Append a prompt to the history ring (bounded, no consecutive dup).
    pub fn remember(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.history.back().map(|s| s.as_str()) == Some(text) {
            return;
        }
        self.history.push_back(text.to_string());
        while self.history.len() > 200 {
            self.history.pop_front();
        }
        self.history_cursor = None;
        self.input_before_history = None;
    }

    /// Move backward (older) through history. Save the live input the
    /// first time so Esc can restore it.
    pub fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let next = match self.history_cursor {
            None => {
                self.input_before_history = Some(self.input.clone());
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(n) => n - 1,
        };
        self.history_cursor = Some(next);
        self.input = self.history[next].clone();
    }

    /// Move forward (newer) through history. Past the newest entry,
    /// restore the live input.
    pub fn history_next(&mut self) {
        let Some(n) = self.history_cursor else { return };
        if n + 1 >= self.history.len() {
            self.history_cursor = None;
            if let Some(saved) = self.input_before_history.take() {
                self.input = saved;
            } else {
                self.input.clear();
            }
        } else {
            self.history_cursor = Some(n + 1);
            self.input = self.history[n + 1].clone();
        }
    }

    pub fn push(&mut self, who: Who, text: impl Into<String>) {
        let line = ChatLine { who, text: text.into() };
        self.log.push_back(line);
        while self.log.len() > self.max_log_lines {
            self.log.pop_front();
        }
    }

    pub fn set_face(&mut self, s: FaceState) {
        self.face = s;
    }

    pub fn tick_once(&mut self) {
        self.tick = self.tick.wrapping_add(1);
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
