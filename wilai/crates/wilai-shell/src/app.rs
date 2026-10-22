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
    /// Timestamp of the current turn's first token (set when busy
    /// flips true, cleared on TurnDone). Used to compute tok/s.
    pub turn_started_at: Option<std::time::Instant>,
    /// Number of streamed TextChunk deltas received in the current
    /// turn. Reset at turn start.
    pub turn_chunk_count: u32,
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
            turn_started_at: None,
            turn_chunk_count: 0,
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

    /// Append streaming text to the most recent Wilai message, or
    /// start a new one if the last line isn't a Wilai turn. Used by
    /// ServerEvent::TextChunk — each token delta extends the live
    /// assistant message as it's being generated.
    pub fn append_wilai_chunk(&mut self, delta: &str) {
        self.turn_chunk_count = self.turn_chunk_count.saturating_add(1);
        if let Some(last) = self.log.back_mut() {
            if matches!(last.who, Who::Wilai) {
                last.text.push_str(delta);
                return;
            }
        }
        self.push(Who::Wilai, delta.to_string());
    }

    /// A one-line status like "Loading model…", "Thinking…",
    /// "Generating 12 ch/s", or an empty string when idle. Rendered
    /// by ui::draw as a subtle hint under the face.
    pub fn busy_status(&self) -> String {
        if !self.busy {
            return String::new();
        }
        let elapsed = self
            .turn_started_at
            .map(|t| t.elapsed().as_secs_f32())
            .unwrap_or(0.0);
        if self.turn_chunk_count == 0 {
            // No tokens yet — the daemon is either loading the model
            // (first prompt after a cold boot) or waiting on tool work.
            if elapsed < 1.0 {
                "Thinking…".to_string()
            } else if elapsed < 8.0 {
                format!("Thinking… {:.0}s", elapsed)
            } else {
                format!("Loading model… {:.0}s", elapsed)
            }
        } else if elapsed > 0.0 {
            let chps = (self.turn_chunk_count as f32) / elapsed;
            format!("Generating {:.0} ch/s", chps)
        } else {
            format!("Generating ({} chunks)", self.turn_chunk_count)
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
