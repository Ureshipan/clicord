//! All rendering. Widgets read from `App` and never mutate it.

use protocol::{Attachment, DirectMessage, GroupMessage};
use ratatui::layout::{Constraint, Direction, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;

use crate::app::{App, LoginField, LoginMode, Screen, Target};
use crate::layout;
use crate::media::{self, ImageArt};

const ACCENT: Color = Color::Cyan;

pub fn render(f: &mut Frame, app: &App) {
    match app.screen {
        Screen::ServerSetup => render_server_setup(f, app),
        Screen::Accounts => render_accounts(f, app),
        Screen::Login => render_login(f, app),
        Screen::Chat => render_chat(f, app),
        Screen::ConnError => render_conn_error(f, app),
    }
    if app.show_help {
        render_help(f);
    }
}

// === Server setup ===========================================================

fn render_server_setup(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(framed(" clicord · server "), area);

    let inner = centered_rect(64, 5, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    f.render_widget(field_line("address", app.server_input.value(), true), rows[0]);
    f.render_widget(
        Paragraph::new(app.status.clone()).style(Style::default().fg(Color::Yellow)),
        rows[2],
    );
    f.render_widget(
        Paragraph::new("Enter: save · Esc: back · e.g. http://host:8080")
            .style(Style::default().fg(Color::DarkGray)),
        rows[3],
    );

    set_field_cursor(f, rows[0], "address", app.server_input.cursor());
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

    let footer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(chunks[1]);
    f.render_widget(
        Paragraph::new(app.status.clone()).style(Style::default().fg(Color::Yellow)),
        footer[0],
    );
    f.render_widget(
        Paragraph::new("↑/↓ select · Enter connect · a add · d delete · q quit · F1 help")
            .style(Style::default().fg(Color::DarkGray)),
        footer[1],
    );
}

// === Login ==================================================================

fn render_login(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(framed(" clicord · login "), area);

    let inner = centered_rect(64, 7, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // mode
            Constraint::Length(1), // spacer
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
    let server = app.config.server.clone().unwrap_or_default();
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(mode, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(format!("  @ {server}"), Style::default().fg(Color::DarkGray)),
        ])),
        rows[0],
    );

    let masked = "*".repeat(app.password_input.len());
    f.render_widget(field_line("user", app.username_input.value(), app.login_field == LoginField::Username), rows[2]);
    f.render_widget(field_line("pass", &masked, app.login_field == LoginField::Password), rows[3]);

    f.render_widget(
        Paragraph::new(app.status.clone()).style(Style::default().fg(Color::Yellow)),
        rows[5],
    );
    f.render_widget(
        Paragraph::new("Tab: field · Ctrl+R: mode · Enter: submit · Esc: back")
            .style(Style::default().fg(Color::DarkGray)),
        rows[6],
    );

    let (label, field, row) = match app.login_field {
        LoginField::Username => ("user", &app.username_input, rows[2]),
        LoginField::Password => ("pass", &app.password_input, rows[3]),
    };
    set_field_cursor(f, row, label, field.cursor());
}

// === Chat ===================================================================

fn render_chat(f: &mut Frame, app: &App) {
    let l = layout::chat_layout(f.area());

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " clicord ",
                Style::default().bg(ACCENT).fg(Color::Black).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(" {} ", app.username)),
            Span::styled(format!("» {}", app.active_name()), Style::default().fg(Color::Green)),
        ])),
        l.header,
    );

    // Conversation list: groups and DMs, with unread badges.
    let items: Vec<ListItem> = app
        .chat_entries()
        .iter()
        .map(|t| {
            let active = app.active.as_ref() == Some(t);
            let (marker, online) = match t {
                Target::Dm(u) => {
                    let on = app.online.contains(u);
                    (if on { "●" } else { "○" }.to_string(), on)
                }
                Target::Group(_) => ("▣".to_string(), false),
            };
            let style = if active {
                Style::default().fg(Color::Black).bg(ACCENT).add_modifier(Modifier::BOLD)
            } else if online {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::Gray)
            };
            let mut spans = vec![Span::styled(format!("{marker} {}", app.target_name(t)), style)];
            if let Some(n) = app.unread.get(t).filter(|n| **n > 0) {
                spans.push(Span::styled(
                    format!(" ({n})"),
                    Style::default().fg(Color::Black).bg(Color::Red).add_modifier(Modifier::BOLD),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    f.render_widget(
        List::new(items).block(Block::default().title(" chats ").borders(Borders::ALL)),
        l.peers,
    );

    // Messages for the active conversation (with date separators & attachments).
    let lines: Vec<Line> = chat_lines(app);
    let title = format!(" {} ", app.active_name());
    let visible = l.messages.height.saturating_sub(2) as usize;
    let total = lines.len();
    // `scroll` counts messages hidden below the viewport (0 = newest visible).
    let bottom = total.saturating_sub(app.scroll);
    let start = bottom.saturating_sub(visible);
    let shown: Vec<Line> = lines[start..bottom].to_vec();
    f.render_widget(
        Paragraph::new(shown)
            .block(Block::default().title(title).borders(Borders::ALL))
            .wrap(Wrap { trim: false }),
        l.messages,
    );

    // When scrolled up, show a marker (and unread count) at the bottom edge.
    if app.scroll > 0 && l.messages.height > 2 {
        let n = app
            .active
            .as_ref()
            .and_then(|t| app.unread.get(t))
            .copied()
            .unwrap_or(0);
        let text = if n > 0 {
            format!(" ▼ {n} new — PgDn / wheel for latest ")
        } else {
            " ▼ PgDn / wheel for latest ".to_string()
        };
        let row = Rect::new(
            l.messages.x + 1,
            l.messages.y + l.messages.height - 2,
            l.messages.width.saturating_sub(2),
            1,
        );
        f.render_widget(Clear, row);
        f.render_widget(
            Paragraph::new(text).style(
                Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            row,
        );
    }

    // Input box — focused, accent border + live cursor. Title doubles as the
    // typing indicator for the active conversation.
    let input_title = match app.typing_text() {
        Some(t) => format!(" {t} "),
        None => " message ".to_string(),
    };
    f.render_widget(
        Paragraph::new(app.input.value()).block(
            Block::default()
                .title(input_title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT)),
        ),
        l.input,
    );
    let cursor_x = (l.input.x + 1 + app.input.cursor() as u16)
        .min(l.input.x + l.input.width.saturating_sub(2));
    f.set_cursor_position(Position::new(cursor_x, l.input.y + 1));

    if !app.suggestions.is_empty() {
        render_suggestions(f, app, l.input);
    }
}

fn render_suggestions(f: &mut Frame, app: &App, input: Rect) {
    let count = app.suggestions.len().min(6) as u16;
    let height = count + 2;
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
            let style = if Some(i) == app.selected {
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

// === Connection error =======================================================

fn render_conn_error(f: &mut Frame, app: &App) {
    let area = f.area();
    f.render_widget(
        Block::default()
            .title(" connection problem ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red)),
        area,
    );

    let inner = centered_rect(70, 5, area);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1), Constraint::Length(1)])
        .split(inner);

    f.render_widget(
        Paragraph::new(app.status.clone())
            .style(Style::default().fg(Color::Yellow))
            .wrap(Wrap { trim: true }),
        rows[0],
    );
    f.render_widget(
        Paragraph::new(Line::from(vec![
            opt("r", "retry"),
            opt("s", "change server"),
            opt("a", "accounts"),
            opt("q", "quit"),
        ])),
        rows[2],
    );
}

fn opt(key: &'static str, label: &'static str) -> Span<'static> {
    Span::styled(
        format!("[{key}] {label}   "),
        Style::default().fg(Color::White),
    )
}

// === Help overlay ===========================================================

fn render_help(f: &mut Frame) {
    let area = f.area();
    let rect = centered_rect(70, 19, area);
    f.render_widget(Clear, rect);

    let line = |k: &str, v: &str| {
        Line::from(vec![
            Span::styled(format!("{k:<22}"), Style::default().fg(ACCENT)),
            Span::raw(v.to_string()),
        ])
    };
    let body = vec![
        line("F1", "toggle this help"),
        line("Ctrl+Q", "quit from anywhere"),
        Line::raw(""),
        line("Accounts", "↑/↓ move · Enter connect · a add · d delete"),
        line("Login", "Tab field · Ctrl+R login/register · Enter submit"),
        Line::raw(""),
        line("/dm <user>", "open a direct chat (or click a name)"),
        line("/find <prefix>", "search registered users"),
        line("/group <name> [users]", "create a group"),
        line("/g <name>", "open one of your groups"),
        line("/file <path>", "send a file/image to the open chat"),
        line("/view <n>", "open attachment n in its default app"),
        line("Tab", "autocomplete commands / names"),
        line("←/→ Home/End", "move caret · Del/Backspace edit"),
        line("PgUp/PgDn ↑/↓ wheel", "scroll history (sticks to newest)"),
        line("/accounts · Esc", "back to session manager"),
        line("/quit", "exit the app"),
    ];

    f.render_widget(
        Paragraph::new(body).block(
            Block::default()
                .title(" help · commands & keys ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(ACCENT)),
        ),
        rect,
    );
}

// === Shared helpers =========================================================

fn framed(title: &'static str) -> Block<'static> {
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
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

/// Place the terminal cursor inside a `field_line` at character `cursor`.
fn set_field_cursor(f: &mut Frame, row: Rect, label: &str, cursor: usize) {
    let prefix = (2 + label.len() + 2) as u16; // "> " + label + ": "
    f.set_cursor_position(Position::new(row.x + prefix + cursor as u16, row.y));
}

/// A message from either kind of conversation, flattened for uniform rendering.
struct MsgView<'a> {
    from: &'a str,
    body: &'a str,
    ts: i64,
    attachment: Option<&'a Attachment>,
}

impl<'a> MsgView<'a> {
    fn from_dm(m: &'a DirectMessage) -> Self {
        Self { from: &m.from, body: &m.body, ts: m.ts, attachment: m.attachment.as_ref() }
    }
    fn from_group(m: &'a GroupMessage) -> Self {
        Self { from: &m.from, body: &m.body, ts: m.ts, attachment: m.attachment.as_ref() }
    }
}

/// Build every rendered line for the active conversation: a `[DD.MM.YY]`
/// separator whenever the day changes, then each message, its attachment line
/// and (for images) an inline thumbnail. Shared with `app` so scroll maths and
/// rendering agree on the line count.
pub fn chat_lines(app: &App) -> Vec<Line<'static>> {
    let msgs: Vec<MsgView> = match &app.active {
        None => return Vec::new(),
        Some(Target::Dm(peer)) => app.dm_messages(peer).into_iter().map(MsgView::from_dm).collect(),
        Some(Target::Group(id)) => app.group_messages(*id).into_iter().map(MsgView::from_group).collect(),
    };

    let mut lines = Vec::new();
    let mut last_day: Option<String> = None;
    let mut att_no = 0usize;
    for m in msgs {
        let day = day_label(m.ts);
        if last_day.as_deref() != Some(day.as_str()) {
            lines.push(date_separator(&day));
            last_day = Some(day);
        }
        lines.push(message_line(app, m.from, m.body, m.ts));
        if let Some(a) = m.attachment {
            att_no += 1;
            lines.push(attachment_line(att_no, a));
            if a.is_image() {
                if let Some(art) = app.image_art(a.id) {
                    lines.extend(thumbnail_lines(art));
                }
            }
        }
    }
    lines
}

fn message_line(app: &App, from: &str, body: &str, ts: i64) -> Line<'static> {
    let who_style = if from == app.username {
        Style::default().fg(ACCENT)
    } else {
        Style::default().fg(Color::Magenta)
    };
    Line::from(vec![
        Span::styled(format!("[{}] ", format_ts(ts)), Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{from}: "), who_style.add_modifier(Modifier::BOLD)),
        Span::raw(body.to_string()),
    ])
}

/// A centered day divider, e.g. `──────  01.07.26  ──────`.
fn date_separator(day: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("──────  {day}  ──────"),
        Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
    ))
    .centered()
}

/// The line describing an attachment beneath its message.
fn attachment_line(no: usize, att: &Attachment) -> Line<'static> {
    let icon = if att.is_image() { "🖼" } else { "📎" };
    Line::from(vec![
        Span::styled(format!("    {icon} [{no}] "), Style::default().fg(Color::Yellow)),
        Span::styled(
            att.name.clone(),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("  ({})  ", media::human_size(att.size)),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(format!("· /view {no}"), Style::default().fg(Color::DarkGray)),
    ])
}

/// Turn a decoded thumbnail into indented half-block lines.
fn thumbnail_lines(art: &ImageArt) -> Vec<Line<'static>> {
    art.rows
        .iter()
        .map(|row| {
            let mut spans = vec![Span::raw("    ".to_string())];
            spans.extend(row.iter().map(|cell| {
                Span::styled(
                    "▀".to_string(),
                    Style::default()
                        .fg(Color::Rgb(cell.top.0, cell.top.1, cell.top.2))
                        .bg(Color::Rgb(cell.bottom.0, cell.bottom.1, cell.bottom.2)),
                )
            }));
            Line::from(spans)
        })
        .collect()
}

fn format_ts(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.format("%H:%M").to_string(),
        None => "--:--".to_string(),
    }
}

fn day_label(ms: i64) -> String {
    use chrono::{Local, TimeZone};
    match Local.timestamp_millis_opt(ms).single() {
        Some(dt) => dt.format("%d.%m.%y").to_string(),
        None => "??.??.??".to_string(),
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
