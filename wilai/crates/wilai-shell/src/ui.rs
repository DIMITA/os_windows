//! Draw the TUI. Layout:
//!
//!   ┌─ status bar ───────────────────────────────────────┐
//!   │ ┌─ face ─┐ ┌─ chat ────────────────────────────┐ │
//!   │ │        │ │ ...                               │ │
//!   │ └────────┘ └───────────────────────────────────┘ │
//!   │ > input ...                                       │
//!   └───────────────────────────────────────────────────┘
//!
//! Over this, a modal popup renders when a confirm is pending.

use crate::app::{App, Who};
use crate::face;
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

pub fn draw(f: &mut Frame<'_>, app: &App) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(5), Constraint::Length(3)])
        .split(f.area());

    draw_status(f, app, outer[0]);
    draw_body(f, app, outer[1]);
    draw_input(f, app, outer[2]);

    if let Some(c) = &app.pending_confirm {
        draw_confirm(f, c, f.area());
    }
}

fn draw_status(f: &mut Frame<'_>, app: &App, area: Rect) {
    let sess = app.session.as_deref().unwrap_or("—");
    let left = format!(" Wilai · local · {} · session={} ", app.mode, sess);
    let right = format!(" {} ", app.status_right);
    let gap = area.width.saturating_sub(left.len() as u16 + right.len() as u16);
    let line = format!("{left}{}{right}", " ".repeat(gap as usize));
    let p = Paragraph::new(line).style(
        Style::default()
            .bg(Color::DarkGray)
            .fg(Color::White)
            .add_modifier(Modifier::BOLD),
    );
    f.render_widget(p, area);
}

fn draw_body(f: &mut Frame<'_>, app: &App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(22), Constraint::Min(20)])
        .split(area);

    draw_face(f, app, cols[0]);
    draw_chat(f, app, cols[1]);
}

fn draw_face(f: &mut Frame<'_>, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(app.face.hue()))
        .title(Span::styled(
            " WILAI ",
            Style::default()
                .fg(app.face.hue())
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines = face::render(app.face, app.tick);
    let p = Paragraph::new(lines).alignment(Alignment::Center);
    f.render_widget(p, inner);
}

fn draw_chat(f: &mut Frame<'_>, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" conversation ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines: Vec<Line<'_>> = Vec::new();
    for cl in &app.log {
        let (color, bold) = match cl.who {
            Who::You => (Color::White, true),
            Who::Wilai => (Color::Cyan, true),
            Who::Tool => (Color::Yellow, false),
            Who::System => (Color::DarkGray, false),
        };
        let tag_style = Style::default().fg(color).add_modifier(if bold {
            Modifier::BOLD
        } else {
            Modifier::empty()
        });
        let text_style = Style::default().fg(color);
        for (i, chunk) in cl.text.lines().enumerate() {
            let tag = if i == 0 {
                format!(" {} │ ", cl.who.tag())
            } else {
                "       │ ".to_string()
            };
            lines.push(Line::from(vec![
                Span::styled(tag, tag_style),
                Span::styled(chunk.to_string(), text_style),
            ]));
        }
    }

    let total = lines.len() as u16;
    let visible = inner.height;
    let max_scroll = total.saturating_sub(visible);
    let effective_scroll = app.scroll.min(max_scroll);

    let p = Paragraph::new(lines)
        .wrap(Wrap { trim: false })
        .scroll((effective_scroll, 0));
    f.render_widget(p, inner);
}

fn draw_input(f: &mut Frame<'_>, app: &App, area: Rect) {
    let hint: String = if app.pending_confirm.is_some() {
        " (y)es / (n)o — Esc to cancel ".to_string()
    } else if app.recording {
        " 🎙 listening… release to transcribe ".to_string()
    } else if app.busy {
        " … working — Esc to interrupt ".to_string()
    } else {
        let mut h = String::from(" Enter to send · Ctrl+C to quit · PgUp/PgDn to scroll");
        if app.voice_enabled {
            if app.voice_can_listen {
                h.push_str(" · F2 talk");
            }
            if app.voice_can_speak {
                let state = if app.voice_mute { "muted" } else { "on" };
                h.push_str(&format!(" · F3 tts [{state}]"));
            }
        }
        h.push(' ');
        h
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(hint.clone(), Style::default().fg(Color::DarkGray)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let prompt = format!("> {}", app.input);
    let p = Paragraph::new(prompt).style(Style::default().fg(Color::White));
    f.render_widget(p, inner);

    // Cursor position: inside the input area, two cols after start for `> `.
    let x = inner.x + 2 + app.input.chars().count() as u16;
    let y = inner.y;
    if !app.pending_confirm.is_some() {
        f.set_cursor_position((x.min(inner.x + inner.width.saturating_sub(1)), y));
    }
}

fn draw_confirm(f: &mut Frame<'_>, c: &crate::app::PendingConfirm, area: Rect) {
    let w = area.width.min(70);
    let h = 7u16;
    let x = (area.width.saturating_sub(w)) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let popup = Rect { x, y, width: w, height: h };
    f.render_widget(Clear, popup);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow))
        .title(Span::styled(
            " confirm ",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(popup);
    f.render_widget(block, popup);
    let hint = if c.default_yes { "[Y/n]" } else { "[y/N]" };
    let body = format!("{}\n\n  {hint}  (y / n)", c.prompt);
    let p = Paragraph::new(body)
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(Color::White));
    f.render_widget(p, inner);
}
