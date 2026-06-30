//! Shared layout geometry so rendering (`ui.rs`) and mouse hit-testing
//! (`app.rs`) agree on where each panel lives.

use ratatui::layout::{Constraint, Direction, Layout, Rect};

pub struct ChatLayout {
    pub header: Rect,
    pub peers: Rect,
    pub messages: Rect,
    pub input: Rect,
}

pub fn chat_layout(area: Rect) -> ChatLayout {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(3), Constraint::Length(3)])
        .split(area);
    let mid = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(22), Constraint::Min(10)])
        .split(v[1]);
    ChatLayout {
        header: v[0],
        peers: mid[0],
        messages: mid[1],
        input: v[2],
    }
}

/// The bordered list of accounts occupies everything except a 2-row hint
/// footer. Items begin one row below the top border.
pub fn accounts_list(area: Rect) -> Rect {
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(2)])
        .split(area)[0]
}
