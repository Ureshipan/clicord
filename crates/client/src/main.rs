//! clicord: terminal client for the clicord messenger.
//!
//! Usage: `clicord [SERVER_URL]`  (default http://127.0.0.1:8080)
//! The server URL is only the default offered when adding a new account;
//! saved accounts remember their own server.

mod app;
mod input;
mod layout;
mod net;
mod session;
mod ui;

use anyhow::Result;
use app::App;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind,
};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use futures_util::StreamExt;
use net::Incoming;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::Terminal;
use std::io::stdout;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<()> {
    let default_server = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;

    let res = run(&mut terminal, default_server).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
    terminal.show_cursor()?;

    res
}

async fn run<B: Backend>(terminal: &mut Terminal<B>, default_server: String) -> Result<()> {
    let config = session::load_config();
    let store = session::load();
    let (in_tx, mut in_rx) = mpsc::unbounded_channel::<Incoming>();
    let mut app = App::new(default_server, config, store, in_tx);
    let mut events = EventStream::new();

    while !app.should_quit {
        terminal.draw(|f| ui::render(f, &app))?;

        tokio::select! {
            maybe_event = events.next() => match maybe_event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => app.on_key(key).await,
                Some(Ok(Event::Mouse(m))) => app.on_mouse(m),
                Some(Ok(_)) => {}
                Some(Err(_)) | None => break,
            },
            Some(incoming) = in_rx.recv() => app.on_incoming(incoming),
        }
    }
    Ok(())
}
