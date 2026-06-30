//! Application state and input handling, kept separate from rendering
//! (`ui.rs`), networking (`net.rs`) and persistence (`session.rs`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use protocol::{ClientMsg, DirectMessage, ServerMsg};
use ratatui::layout::Rect;
use std::collections::BTreeSet;
use tokio::sync::mpsc::UnboundedSender;

use crate::session::{self, Account, Store};
use crate::{layout, net};

pub enum Screen {
    Accounts,
    Login,
    Chat,
}

#[derive(Clone, Copy)]
pub enum LoginMode {
    Login,
    Register,
}

#[derive(Clone, Copy, PartialEq)]
pub enum LoginField {
    Server,
    Username,
    Password,
}

const COMMANDS: &[&str] = &["dm", "quit", "help"];

pub struct App {
    pub default_server: String,
    pub screen: Screen,
    pub should_quit: bool,
    pub status: String,

    // account manager
    pub store: Store,
    pub accounts_idx: usize,

    // login screen
    pub login_mode: LoginMode,
    pub login_field: LoginField,
    pub login_server: String,
    pub username_input: String,
    pub password_input: String,

    // chat screen
    pub username: String,
    pub server: String,
    pub authed: bool,
    pub active_peer: String,
    pub input: String,
    pub messages: Vec<DirectMessage>,
    pub peers: BTreeSet<String>,
    pub online: BTreeSet<String>,

    // autocomplete
    pub suggestions: Vec<String>,
    pub suggestion_idx: usize,

    // networking
    in_tx: UnboundedSender<ServerMsg>,
    out_tx: Option<UnboundedSender<ClientMsg>>,
}

impl App {
    pub fn new(default_server: String, store: Store, in_tx: UnboundedSender<ServerMsg>) -> Self {
        let screen = if store.accounts.is_empty() {
            Screen::Login
        } else {
            Screen::Accounts
        };
        let status = if store.accounts.is_empty() {
            "no saved accounts — register or log in".into()
        } else {
            "select an account · Enter connect · a add · d delete".into()
        };
        Self {
            login_server: default_server.clone(),
            default_server,
            screen,
            should_quit: false,
            status,
            store,
            accounts_idx: 0,
            login_mode: LoginMode::Login,
            login_field: LoginField::Username,
            username_input: String::new(),
            password_input: String::new(),
            username: String::new(),
            server: String::new(),
            authed: false,
            active_peer: String::new(),
            input: String::new(),
            messages: Vec::new(),
            peers: BTreeSet::new(),
            online: BTreeSet::new(),
            suggestions: Vec::new(),
            suggestion_idx: 0,
            in_tx,
            out_tx: None,
        }
    }

    // === Key handling =======================================================

    pub async fn on_key(&mut self, key: KeyEvent) {
        match self.screen {
            Screen::Accounts => self.accounts_key(key),
            Screen::Login => self.login_key(key).await,
            Screen::Chat => self.chat_key(key),
        }
    }

    // --- Accounts screen ----------------------------------------------------

    fn accounts_key(&mut self, key: KeyEvent) {
        let n = self.store.accounts.len();
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Up | KeyCode::Char('k') => {
                if n > 0 {
                    self.accounts_idx = (self.accounts_idx + n - 1) % n;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if n > 0 {
                    self.accounts_idx = (self.accounts_idx + 1) % n;
                }
            }
            KeyCode::Enter => self.connect_selected(),
            KeyCode::Char('a') => self.open_login(),
            KeyCode::Char('d') => {
                if n > 0 {
                    self.store.remove(self.accounts_idx);
                    session::save(&self.store);
                    if self.accounts_idx >= self.store.accounts.len() {
                        self.accounts_idx = self.store.accounts.len().saturating_sub(1);
                    }
                    self.status = "account removed".into();
                    if self.store.accounts.is_empty() {
                        self.open_login();
                    }
                }
            }
            _ => {}
        }
    }

    fn open_login(&mut self) {
        self.screen = Screen::Login;
        self.login_mode = LoginMode::Login;
        self.login_field = LoginField::Username;
        self.login_server = self.default_server.clone();
        self.username_input.clear();
        self.password_input.clear();
        self.status = "enter credentials · Ctrl+R toggles login/register".into();
    }

    fn connect_selected(&mut self) {
        if let Some(a) = self.store.accounts.get(self.accounts_idx).cloned() {
            self.connect(a.server, a.username, a.token);
        }
    }

    // --- Login screen -------------------------------------------------------

    async fn login_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                if self.store.accounts.is_empty() {
                    self.should_quit = true;
                } else {
                    self.screen = Screen::Accounts;
                }
            }
            KeyCode::Tab => {
                self.login_field = match self.login_field {
                    LoginField::Server => LoginField::Username,
                    LoginField::Username => LoginField::Password,
                    LoginField::Password => LoginField::Server,
                }
            }
            KeyCode::Char('r') | KeyCode::Char('R') if ctrl => {
                self.login_mode = match self.login_mode {
                    LoginMode::Login => LoginMode::Register,
                    LoginMode::Register => LoginMode::Login,
                }
            }
            KeyCode::Enter => self.submit_login().await,
            KeyCode::Backspace => {
                self.login_field_mut().pop();
            }
            KeyCode::Char(c) if !ctrl => self.login_field_mut().push(c),
            _ => {}
        }
    }

    fn login_field_mut(&mut self) -> &mut String {
        match self.login_field {
            LoginField::Server => &mut self.login_server,
            LoginField::Username => &mut self.username_input,
            LoginField::Password => &mut self.password_input,
        }
    }

    async fn submit_login(&mut self) {
        let path = match self.login_mode {
            LoginMode::Login => "login",
            LoginMode::Register => "register",
        };
        self.status = "connecting…".into();
        match net::auth(&self.login_server, path, &self.username_input, &self.password_input).await {
            Ok(resp) => {
                self.store.upsert(Account {
                    server: self.login_server.clone(),
                    username: resp.username.clone(),
                    token: resp.token.clone(),
                });
                session::save(&self.store);
                self.password_input.clear();
                self.connect(self.login_server.clone(), resp.username, resp.token);
            }
            Err(e) => self.status = format!("auth failed: {e}"),
        }
    }

    // --- Chat screen --------------------------------------------------------

    fn chat_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.should_quit = true,
            KeyCode::Tab => self.apply_suggestion(),
            KeyCode::Enter => {
                self.submit_chat();
                self.recompute_suggestions();
            }
            KeyCode::Backspace => {
                self.input.pop();
                self.recompute_suggestions();
            }
            KeyCode::Char(c) if !ctrl => {
                self.input.push(c);
                self.recompute_suggestions();
            }
            _ => {}
        }
    }

    fn submit_chat(&mut self) {
        let line = std::mem::take(&mut self.input);
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        if let Some(rest) = line.strip_prefix('/') {
            self.handle_command(rest);
            return;
        }
        if self.active_peer.is_empty() {
            self.status = "no active chat — use /dm <user> or click a name".into();
            return;
        }
        if let Some(tx) = &self.out_tx {
            let _ = tx.send(ClientMsg::SendDm {
                to: self.active_peer.clone(),
                body: line.to_string(),
            });
        }
    }

    fn handle_command(&mut self, cmd: &str) {
        let mut parts = cmd.split_whitespace();
        match parts.next() {
            Some("dm") | Some("to") => match parts.next() {
                Some(user) => self.open_chat_with(user.to_string()),
                None => self.status = "usage: /dm <user>".into(),
            },
            Some("quit") | Some("q") => self.should_quit = true,
            Some("help") => self.status = "/dm <user> · /quit · Tab completes · click a name".into(),
            _ => self.status = "unknown command (try /help)".into(),
        }
    }

    fn open_chat_with(&mut self, user: String) {
        self.peers.insert(user.clone());
        self.active_peer = user;
        self.status = format!("now chatting with {}", self.active_peer);
    }

    // --- Autocomplete -------------------------------------------------------

    fn recompute_suggestions(&mut self) {
        self.suggestions.clear();
        self.suggestion_idx = 0;
        let Some(rest) = self.input.strip_prefix('/') else {
            return;
        };
        match rest.split_once(' ') {
            // Still typing the command word.
            None => {
                for c in COMMANDS {
                    if c.starts_with(rest) {
                        self.suggestions.push(format!("/{c}"));
                    }
                }
            }
            // Completing an argument.
            Some((cmd, arg)) if cmd == "dm" || cmd == "to" => {
                for u in self.known_users() {
                    if u != self.username && u.starts_with(arg) {
                        self.suggestions.push(format!("/{cmd} {u}"));
                    }
                }
            }
            Some(_) => {}
        }
    }

    fn known_users(&self) -> BTreeSet<String> {
        self.peers.union(&self.online).cloned().collect()
    }

    fn apply_suggestion(&mut self) {
        if self.suggestions.is_empty() {
            return;
        }
        self.input = self.suggestions[self.suggestion_idx].clone();
        self.suggestion_idx = (self.suggestion_idx + 1) % self.suggestions.len();
    }

    // === Mouse handling =====================================================

    pub fn on_mouse(&mut self, ev: MouseEvent) {
        if ev.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        let (w, h) = crossterm::terminal::size().unwrap_or((80, 24));
        let area = Rect::new(0, 0, w, h);
        match self.screen {
            Screen::Chat => {
                let peers = layout::chat_layout(area).peers;
                if let Some(peer) = peer_at(&self.peers, peers, ev.column, ev.row) {
                    self.open_chat_with(peer);
                }
            }
            Screen::Accounts => {
                let list = layout::accounts_list(area);
                // Items start one row below the top border.
                if ev.row > list.y && ev.row < list.y + list.height.saturating_sub(1) {
                    let idx = (ev.row - list.y - 1) as usize;
                    if idx < self.store.accounts.len() {
                        self.accounts_idx = idx;
                        self.connect_selected();
                    }
                }
            }
            Screen::Login => {}
        }
    }

    // === Incoming server events ============================================

    pub fn on_server_msg(&mut self, msg: ServerMsg) {
        match msg {
            ServerMsg::AuthOk { username } => {
                self.authed = true;
                self.status = format!("online as {username} · /help for commands");
            }
            ServerMsg::History { messages } => {
                for m in messages {
                    self.note_peer(&m);
                    self.messages.push(m);
                }
            }
            ServerMsg::Dm(m) => {
                self.note_peer(&m);
                if self.active_peer.is_empty() {
                    self.active_peer = self.other_party(&m);
                }
                self.messages.push(m);
            }
            ServerMsg::Presence { username, online } => {
                if online {
                    self.online.insert(username);
                } else {
                    self.online.remove(&username);
                }
            }
            ServerMsg::Error { message } => {
                // A failure before we ever authenticated means a bad/expired
                // token — drop back to the account picker.
                if !self.authed && matches!(self.screen, Screen::Chat) {
                    self.out_tx = None;
                    self.screen = if self.store.accounts.is_empty() {
                        Screen::Login
                    } else {
                        Screen::Accounts
                    };
                    self.status = format!("session ended ({message}) — log in again");
                } else {
                    self.status = format!("server: {message}");
                }
            }
            ServerMsg::Pong => {}
        }
    }

    fn other_party(&self, m: &DirectMessage) -> String {
        if m.from == self.username {
            m.to.clone()
        } else {
            m.from.clone()
        }
    }

    fn note_peer(&mut self, m: &DirectMessage) {
        let peer = self.other_party(m);
        if !peer.is_empty() {
            self.peers.insert(peer);
        }
    }

    /// Messages belonging to the currently open conversation, oldest first.
    pub fn messages_for_active(&self) -> Vec<&DirectMessage> {
        if self.active_peer.is_empty() {
            return Vec::new();
        }
        self.messages
            .iter()
            .filter(|m| {
                (m.from == self.username && m.to == self.active_peer)
                    || (m.from == self.active_peer && m.to == self.username)
            })
            .collect()
    }

    // === Session wiring =====================================================

    fn connect(&mut self, server: String, username: String, token: String) {
        self.username = username;
        self.server = server.clone();
        self.authed = false;
        self.active_peer.clear();
        self.messages.clear();
        self.peers.clear();
        self.online.clear();
        self.input.clear();
        self.suggestions.clear();
        self.suggestion_idx = 0;
        self.out_tx = Some(net::spawn_ws(server, token, self.in_tx.clone()));
        self.screen = Screen::Chat;
        self.status = "connecting…".into();
    }
}

/// Map a click inside the (bordered) peers panel to a peer name.
fn peer_at(peers: &BTreeSet<String>, rect: Rect, col: u16, row: u16) -> Option<String> {
    let inside_x = col >= rect.x && col < rect.x + rect.width;
    let inside_y = row > rect.y && row < rect.y + rect.height.saturating_sub(1);
    if !inside_x || !inside_y {
        return None;
    }
    let idx = (row - rect.y - 1) as usize;
    peers.iter().nth(idx).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> App {
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        App::new("http://x".into(), Store::default(), tx)
    }

    #[test]
    fn completes_command_names() {
        let mut a = test_app();
        a.input = "/d".into();
        a.recompute_suggestions();
        assert_eq!(a.suggestions, vec!["/dm"]);
    }

    #[test]
    fn completes_usernames_by_prefix_excluding_self() {
        let mut a = test_app();
        a.username = "me".into();
        for u in ["alice", "alex", "bob", "me"] {
            a.peers.insert(u.into());
        }
        a.input = "/dm al".into();
        a.recompute_suggestions();
        // BTreeSet order: "alex" < "alice"; "me" excluded.
        assert_eq!(a.suggestions, vec!["/dm alex", "/dm alice"]);
    }

    #[test]
    fn tab_cycles_through_suggestions() {
        let mut a = test_app();
        a.peers.insert("alice".into());
        a.peers.insert("alex".into());
        a.input = "/dm al".into();
        a.recompute_suggestions();
        a.apply_suggestion();
        assert_eq!(a.input, "/dm alex");
        a.apply_suggestion();
        assert_eq!(a.input, "/dm alice");
        a.apply_suggestion();
        assert_eq!(a.input, "/dm alex"); // wraps around
    }

    #[test]
    fn no_suggestions_without_slash() {
        let mut a = test_app();
        a.input = "hello".into();
        a.recompute_suggestions();
        assert!(a.suggestions.is_empty());
    }
}
