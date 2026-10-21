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
}

impl Who {
    pub fn tag(&self) -> &'static str {
        match self {
            Who::You => "you  ",
            Who::Wilai => "wilai",
            Who::Tool => "tool ",
            Who::System => "sys  ",
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
