//! ASCII rendering of the Wilai robot face per state. The frames mirror
//! the Aurora HUD artifact's state model: idle / listening / thinking /
//! speaking / acting / alert / offline. Each frame is a 13-wide × 9-tall
//! block of chars; the renderer picks a frame by state and animates the
//! "scan line" position with a counter from the UI loop.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaceState {
    Idle,
    /// Reached once v3.6 wires voice/wake-word into the shell.
    #[allow(dead_code)]
    Listening,
    Thinking,
    Speaking,
    Acting,
    Alert,
    Offline,
}

impl FaceState {
    pub fn label(&self) -> &'static str {
        match self {
            FaceState::Idle => "IDLE",
            FaceState::Listening => "LISTEN",
            FaceState::Thinking => "THINK",
            FaceState::Speaking => "SPEAK",
            FaceState::Acting => "ACT",
            FaceState::Alert => "ALERT",
            FaceState::Offline => "OFFLINE",
        }
    }

    pub fn hue(&self) -> Color {
        match self {
            FaceState::Idle => Color::Cyan,
            FaceState::Listening => Color::LightCyan,
            FaceState::Thinking => Color::LightBlue,
            FaceState::Speaking => Color::LightMagenta,
            FaceState::Acting => Color::LightGreen,
            FaceState::Alert => Color::LightRed,
            FaceState::Offline => Color::DarkGray,
        }
    }
}

/// Which eye glyph to use for a given state + tick (for subtle pulsation).
fn eyes(state: FaceState, tick: u64) -> &'static str {
    match state {
        FaceState::Idle => {
            if tick % 60 < 55 { "●─●" } else { "·─·" }
        }
        FaceState::Listening => "◉─◉",
        FaceState::Thinking => {
            match tick % 4 {
                0 => "◐─◑",
                1 => "◓─◒",
                2 => "◑─◐",
                _ => "◒─◓",
            }
        }
        FaceState::Speaking => {
            if tick % 2 == 0 { "◉─◉" } else { "◎─◎" }
        }
        FaceState::Acting => "✦─✦",
        FaceState::Alert => {
            if tick % 2 == 0 { "✕─✕" } else { "×─×" }
        }
        FaceState::Offline => "∘─∘",
    }
}

/// Lower-face chin / mouth glyph.
fn chin(state: FaceState, tick: u64) -> &'static str {
    match state {
        FaceState::Speaking => {
            match tick % 4 {
                0 => "┈┈┈",
                1 => "┉┉┉",
                2 => "═══",
                _ => "┉┉┉",
            }
        }
        FaceState::Alert => "▂▂▂",
        FaceState::Offline => "···",
        _ => "───",
    }
}

/// Draw a thin HUD ring around the head. Four cardinals rotate on `tick`
/// to give the "scanning" feel from the artifact.
fn cardinals(state: FaceState, tick: u64) -> [&'static str; 4] {
    // N, E, S, W markers
    match state {
        FaceState::Acting => {
            // Sweep: one bright, others dim; brightest rotates clockwise.
            let i = (tick / 2) as usize % 4;
            let bright = ["◆", "◆", "◆", "◆"];
            let dim = ["◇", "◇", "◇", "◇"];
            let mut out = dim;
            out[i] = bright[i];
            out
        }
        FaceState::Alert => {
            if tick % 2 == 0 { ["◆", "◆", "◆", "◆"] } else { ["◇", "◇", "◇", "◇"] }
        }
        FaceState::Offline => ["·", "·", "·", "·"],
        _ => ["·", "·", "·", "·"],
    }
}

/// Build the 9-line face as styled ratatui lines. Width is fixed at 15.
pub fn render(state: FaceState, tick: u64) -> Vec<Line<'static>> {
    let fg = state.hue();
    let dim = Style::default().fg(Color::DarkGray);
    let main = Style::default().fg(fg);
    let bold = Style::default().fg(fg).add_modifier(Modifier::BOLD);

    let eyes = eyes(state, tick);
    let chin = chin(state, tick);
    let [n, e, s, w] = cardinals(state, tick);

    let scan = if matches!(state, FaceState::Listening) {
        // scan line sweeps rows 1..7 top-to-bottom then wraps.
        (tick / 2) as usize % 7
    } else {
        usize::MAX
    };

    let row = |idx: usize, left: &'static str, mid: String, right: &'static str| -> Line<'static> {
        let left_sp = Span::styled(left.to_string(), dim);
        let right_sp = Span::styled(right.to_string(), dim);
        let mid_style = if idx == scan { Style::default().fg(Color::White).add_modifier(Modifier::BOLD) } else { main };
        Line::from(vec![left_sp, Span::styled(mid, mid_style), right_sp])
    };

    vec![
        row(0, "  ", format!("╭────{n}────╮"), "  "),
        row(1, "  ", "│         │".to_string(), "  "),
        row(2, "  ", "│ ╭─────╮ │".to_string(), "  "),
        row(3, &if matches!(state, FaceState::Acting) { "◀ " } else { "  " }, format!("│ │{eyes}│ │"), &if matches!(state, FaceState::Acting) { " ▶" } else { "  " }),
        row(4, "  ", format!("│ │ {chin} │ │"), "  "),
        row(5, "  ", "│ ╰─────╯ │".to_string(), "  "),
        row(6, "  ", "│         │".to_string(), "  "),
        row(7, "  ", format!("╰────{s}────╯"), "  "),
        {
            let label = format!("  « {} »  ", state.label());
            Line::from(Span::styled(label, bold))
        },
        // discard side cardinals rendering above by using w/e below as a sub-line
        Line::from(vec![
            Span::styled(format!(" {w} "), dim),
            Span::styled(format!(" {} ", state.label()), bold),
            Span::styled(format!(" {e} "), dim),
        ]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_states_render_without_panic() {
        for s in [
            FaceState::Idle,
            FaceState::Listening,
            FaceState::Thinking,
            FaceState::Speaking,
            FaceState::Acting,
            FaceState::Alert,
            FaceState::Offline,
        ] {
            for tick in 0..8 {
                let lines = render(s, tick);
                assert!(!lines.is_empty());
            }
        }
    }
}
