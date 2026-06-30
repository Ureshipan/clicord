//! clicord: terminal client for the clicord messenger.
//!
//! Usage: `clicord [SERVER_URL]`  (default http://127.0.0.1:8080)

mod app;
mod net;
mod ui;

use anyhow::Result;
use app::App;
use crossterm::event::{Event, EventStream, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use futures_util::StreamExt;
use protocol::ServerMsg;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::Terminal;
use std::io::stdout;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<()> {
    let server = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let res = run(&mut terminal, server).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    res
}

async fn run<B: Backend>(terminal: &mut Terminal<B>, server: String) -> Result<()> {
    // net -> ui channel. We keep `in_tx` inside `App`, so `in_rx.recv()` never
    // returns None and the select branch stays live for the whole session.
    let (in_tx, mut in_rx) = mpsc::unbounded_channel::<ServerMsg>();
    let mut app = App::new(server, in_tx);
    let mut events = EventStream::new();

    while !app.should_quit {
        terminal.draw(|f| ui::render(f, &app))?;

        tokio::select! {
            maybe_event = events.next() => match maybe_event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => app.on_key(key).await,
                Some(Ok(_)) => {}
                Some(Err(_)) | None => break,
            },
            Some(server_msg) = in_rx.recv() => app.on_server_msg(server_msg),
        }
    }
    Ok(())
}
