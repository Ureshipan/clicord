//! All rendering lives here. Widgets read from `App` but never mutate it, so
//! the visual layer stays swappable.

use protocol::DirectMessage;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, LoginField, LoginMode, Screen};

pub fn render(f: &mut Frame, app: &App) {
    match app.screen {
        Screen::Login => render_login(f, app),
        Screen::Chat => render_chat(f, app),
    }
}

fn render_login(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(
        Block::default().title(" clicord ").borders(Borders::ALL),
        area,
    );

    let inner = centered_rect(60, 9, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // mode
            Constraint::Length(1), // spacer
            Constraint::Length(1), // username
            Constraint::Length(1), // password
            Constraint::Length(1), // spacer
            Constraint::Length(1), // status
            Constraint::Length(1), // hints
        ])
        .split(inner);

    let mode = match app.login_mode {
        LoginMode::Login => "LOGIN",
        LoginMode::Register => "REGISTER",
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("mode: "),
            Span::styled(
                mode,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
        ])),
        rows[0],
    );

    f.render_widget(
        field_line("user", &app.username_input, matches!(app.login_field, LoginField::Username)),
        rows[2],
    );
    let masked = "*".repeat(app.password_input.chars().count());
    f.render_widget(
        field_line("pass", &masked, matches!(app.login_field, LoginField::Password)),
        rows[3],
    );

    f.render_widget(
        Paragraph::new(app.status.clone()).style(Style::default().fg(Color::Yellow)),
        rows[5],
    );
    f.render_widget(
        Paragraph::new("Tab: field   Ctrl+R: mode   Enter: submit   Esc: quit")
            .style(Style::default().fg(Color::DarkGray)),
        rows[6],
    );
}

fn field_line(label: &str, value: &str, focused: bool) -> Paragraph<'static> {
    let (marker, style) = if focused {
        ("> ", Style::default().fg(Color::White).add_modifier(Modifier::BOLD))
    } else {
        ("  ", Style::default().fg(Color::Gray))
    };
    Paragraph::new(Line::from(vec![
        Span::styled(format!("{marker}{label}: "), style),
        Span::raw(value.to_string()),
    ]))
}

fn render_chat(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3), Constraint::Length(3)])
        .split(area);

    // Header bar
    let peer = if app.active_peer.is_empty() {
        "(no chat)".to_string()
    } else {
        app.active_peer.clone()
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " clicord ",
                Style::default().bg(Color::Cyan).fg(Color::Black).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {} ", app.username)),
            Span::styled(format!("» {peer}"), Style::default().fg(Color::Green)),
        ])),
        chunks[0],
    );

    // Middle: peers list | messages
    let mid = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(22), Constraint::Min(10)])
        .split(chunks[1]);

    let items: Vec<ListItem> = app
        .peers
        .iter()
        .map(|p| {
            let online = app.online.contains(p);
            let style = if *p == app.active_peer {
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
            } else if online {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Gray)
            };
            let dot = if online { "●" } else { "○" };
            ListItem::new(Line::from(vec![Span::styled(format!("{dot} {p}"), style)]))
        })
        .collect();
    f.render_widget(
        List::new(items).block(Block::default().title(" chats ").borders(Borders::ALL)),
        mid[0],
    );

    let title = if app.active_peer.is_empty() {
        " messages ".to_string()
    } else {
        format!(" {} ", app.active_peer)
    };
    let all: Vec<Line> = app
        .messages_for_active()
        .into_iter()
        .map(|m| format_msg(app, m))
        .collect();
    // Keep the most recent lines visible (poor-man's autoscroll).
    let visible = mid[1].height.saturating_sub(2) as usize;
    let skip = all.len().saturating_sub(visible);
    let shown: Vec<Line> = all.into_iter().skip(skip).collect();
    f.render_widget(
        Paragraph::new(shown)
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        mid[1],
    );

    // Input box
    f.render_widget(
        Paragraph::new(app.input.as_str())
            .block(Block::default().title(format!(" {} ", app.status)).borders(Borders::ALL)),
        chunks[2],
    );
}

fn format_msg(app: &App, m: &DirectMessage) -> Line<'static> {
    let mine = m.from == app.username;
    let who_style = if mine {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::Magenta)
    };
    Line::from(vec![
        Span::styled(format!("[{}] ", format_ts(m.ts)), Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{}: ", m.from), who_style.add_modifier(Modifier::BOLD)),
        Span::raw(m.body.clone()),
    ])
}

fn format_ts(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.format("%H:%M").to_string(),
        None => "--:--".to_string(),
    }
}

/// A rectangle `width_pct`% wide and `height` rows tall, centered in `area`.
fn centered_rect(width_pct: u16, height: u16, area: Rect) -> Rect {
    let w = (area.width * width_pct / 100).max(1);
    let h = height.min(area.height);
    Rect {
        x: area.x + area.width.saturating_sub(w) / 2,
        y: area.y + area.height.saturating_sub(h) / 2,
        width: w,
        height: h,
    }
}
