//! All rendering. Widgets read from `App` and never mutate it.

use protocol::DirectMessage;
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, LoginField, LoginMode, Screen};
use crate::layout;

const ACCENT: Color = Color::Cyan;

pub fn render(f: &mut Frame, app: &App) {
    match app.screen {
        Screen::Accounts => render_accounts(f, app),
        Screen::Login => render_login(f, app),
        Screen::Chat => render_chat(f, app),
    }
}

// === Accounts ===============================================================

fn render_accounts(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area);

    let items: Vec<ListItem> = app
        .store
        .accounts
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let selected = i == app.accounts_idx;
            let style = if selected {
                Style::default().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            let marker = if selected { "▶ " } else { "  " };
            ListItem::new(Line::from(Span::styled(
                format!("{marker}{}  @  {}", a.username, a.server),
                style,
            )))
        })
        .collect();

    f.render_widget(
        List::new(items).block(
            Block::default()
                .title(" accounts ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT)),
        ),
        chunks[0],
    );

    let hints = Line::from(vec![
        Span::styled(&app.status, Style::default().fg(Color::Yellow)),
    ]);
    let footer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(chunks[1]);
    f.render_widget(Paragraph::new(hints), footer[0]);
    f.render_widget(
        Paragraph::new("↑/↓ select · Enter connect · a add · d delete · q quit · click a row")
            .style(Style::default().fg(Color::DarkGray)),
        footer[1],
    );
}

// === Login ==================================================================

fn render_login(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(
        Block::default()
            .title(" clicord · login ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT)),
        area,
    );

    let inner = centered_rect(64, 9, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // mode
            Constraint::Length(1), // spacer
            Constraint::Length(1), // server
            Constraint::Length(1), // user
            Constraint::Length(1), // pass
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
            Span::styled(mode, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        ])),
        rows[0],
    );

    let masked = "*".repeat(app.password_input.chars().count());
    f.render_widget(field_line("server", &app.login_server, app.login_field == LoginField::Server), rows[2]);
    f.render_widget(field_line("user", &app.username_input, app.login_field == LoginField::Username), rows[3]);
    f.render_widget(field_line("pass", &masked, app.login_field == LoginField::Password), rows[4]);

    f.render_widget(
        Paragraph::new(app.status.clone()).style(Style::default().fg(Color::Yellow)),
        rows[6],
    );
    f.render_widget(
        Paragraph::new("Tab: field · Ctrl+R: mode · Enter: submit · Esc: back")
            .style(Style::default().fg(Color::DarkGray)),
        rows[7],
    );

    // Place the real cursor at the end of the focused field.
    let (label, len, row) = match app.login_field {
        LoginField::Server => ("server", app.login_server.chars().count(), rows[2]),
        LoginField::Username => ("user", app.username_input.chars().count(), rows[3]),
        LoginField::Password => ("pass", masked.chars().count(), rows[4]),
    };
    let prefix = (2 + label.len() + 2) as u16; // "> " + label + ": "
    f.set_cursor_position(Position::new(row.x + prefix + len as u16, row.y));
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

// === Chat ===================================================================

fn render_chat(f: &mut Frame, app: &App) {
    let l = layout::chat_layout(f.area());

    // Header
    let peer = if app.active_peer.is_empty() {
        "(no chat)".to_string()
    } else {
        app.active_peer.clone()
    };
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " clicord ",
                Style::default().bg(ACCENT).fg(Color::Black).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {} ", app.username)),
            Span::styled(format!("» {peer}"), Style::default().fg(Color::Green)),
        ])),
        l.header,
    );

    // Peers list
    let items: Vec<ListItem> = app
        .peers
        .iter()
        .map(|p| {
            let online = app.online.contains(p);
            let style = if *p == app.active_peer {
                Style::default().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)
            } else if online {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Gray)
            };
            let dot = if online { "●" } else { "○" };
            ListItem::new(Line::from(Span::styled(format!("{dot} {p}"), style)))
        })
        .collect();
    f.render_widget(
        List::new(items).block(Block::default().title(" chats ").borders(Borders::ALL)),
        l.peers,
    );

    // Messages
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
    let visible = l.messages.height.saturating_sub(2) as usize;
    let skip = all.len().saturating_sub(visible);
    let shown: Vec<Line> = all.into_iter().skip(skip).collect();
    f.render_widget(
        Paragraph::new(shown)
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        l.messages,
    );

    // Input box — focused, so accent border + live cursor.
    f.render_widget(
        Paragraph::new(app.input.as_str()).block(
            Block::default()
                .title(" message ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT)),
        ),
        l.input,
    );
    let cursor_x = (l.input.x + 1 + app.input.chars().count() as u16)
        .min(l.input.x + l.input.width.saturating_sub(2));
    f.set_cursor_position(Position::new(cursor_x, l.input.y + 1));

    // Autocomplete popup, floating just above the input.
    if !app.suggestions.is_empty() {
        render_suggestions(f, app, l.input);
    }
}

fn render_suggestions(f: &mut Frame, app: &App, input: Rect) {
    let count = app.suggestions.len().min(6) as u16;
    let height = count + 2; // borders
    if input.y < height {
        return;
    }
    let rect = Rect::new(input.x, input.y - height, input.width.min(40), height);

    let items: Vec<ListItem> = app
        .suggestions
        .iter()
        .take(6)
        .enumerate()
        .map(|(i, s)| {
            let style = if i == app.suggestion_idx {
                Style::default().fg(Color::Black).bg(ACCENT)
            } else {
                Style::default().fg(Color::Gray)
            };
            ListItem::new(Span::styled(s.clone(), style))
        })
        .collect();

    f.render_widget(Clear, rect);
    f.render_widget(
        List::new(items).block(
            Block::default()
                .title(" Tab ⇥ ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        ),
        rect,
    );
}

fn format_msg(app: &App, m: &DirectMessage) -> Line<'static> {
    let mine = m.from == app.username;
    let who_style = if mine {
        Style::default().fg(ACCENT)
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
