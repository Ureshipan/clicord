//! Application state and input handling, kept separate from rendering
//! (`ui.rs`), networking (`net.rs`) and persistence (`session.rs`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use protocol::{ClientMsg, DirectMessage, ServerMsg};
use ratatui::layout::Rect;
use std::collections::{BTreeMap, BTreeSet};
use tokio::sync::mpsc::UnboundedSender;

use crate::input::TextInput;
use crate::net::{self, Incoming};
use crate::session::{self, Account, Config, Store};
use crate::layout;

pub enum Screen {
    ServerSetup,
    Accounts,
    Login,
    Chat,
    ConnError,
}

/// Where the server-setup screen returns to once an address is entered.
#[derive(Clone, Copy)]
enum SetupReturn {
    Start,
    Reconnect,
}

#[derive(Clone, Copy)]
pub enum LoginMode {
    Login,
    Register,
}

#[derive(Clone, Copy, PartialEq)]
pub enum LoginField {
    Username,
    Password,
}

#[derive(Clone)]
struct PendingConn {
    server: String,
    username: String,
    token: String,
}

const COMMANDS: &[&str] = &["dm", "accounts", "help", "quit"];

pub struct App {
    pub config: Config,
    pub store: Store,
    pub screen: Screen,
    pub should_quit: bool,
    pub status: String,
    pub show_help: bool,

    setup_return: SetupReturn,

    // account manager
    pub accounts_idx: usize,

    // server setup
    pub server_input: TextInput,

    // login
    pub login_mode: LoginMode,
    pub login_field: LoginField,
    pub username_input: TextInput,
    pub password_input: TextInput,

    // chat
    pub username: String,
    pub server: String,
    pub authed: bool,
    pub active_peer: String,
    pub input: TextInput,
    pub messages: Vec<DirectMessage>,
    pub peers: BTreeSet<String>,
    pub online: BTreeSet<String>,
    pub unread: BTreeMap<String, u32>,

    // autocomplete
    pub suggestions: Vec<String>,
    pub selected: Option<usize>,

    // connection retry context
    pending: Option<PendingConn>,
    auth_failed: bool,

    in_tx: UnboundedSender<Incoming>,
    out_tx: Option<UnboundedSender<ClientMsg>>,
}

impl App {
    pub fn new(default_server: String, config: Config, store: Store, in_tx: UnboundedSender<Incoming>) -> Self {
        let (screen, status, server_input) = if config.server.is_none() {
            (
                Screen::ServerSetup,
                "first run — enter the clicord server address".to_string(),
                TextInput::with(default_server),
            )
        } else if store.accounts.is_empty() {
            (
                Screen::Login,
                "no saved accounts — register or log in".to_string(),
                TextInput::default(),
            )
        } else {
            (
                Screen::Accounts,
                "select an account · Enter connect · a add · d delete".to_string(),
                TextInput::default(),
            )
        };

        Self {
            config,
            store,
            screen,
            should_quit: false,
            status,
            show_help: false,
            setup_return: SetupReturn::Start,
            accounts_idx: 0,
            server_input,
            login_mode: LoginMode::Login,
            login_field: LoginField::Username,
            username_input: TextInput::default(),
            password_input: TextInput::default(),
            username: String::new(),
            server: String::new(),
            authed: false,
            active_peer: String::new(),
            input: TextInput::default(),
            messages: Vec::new(),
            peers: BTreeSet::new(),
            online: BTreeSet::new(),
            unread: BTreeMap::new(),
            suggestions: Vec::new(),
            selected: None,
            pending: None,
            auth_failed: false,
            in_tx,
            out_tx: None,
        }
    }

    // === Key handling =======================================================

    pub async fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        // Global: quit and help work from any screen.
        if ctrl && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q')) {
            self.should_quit = true;
            return;
        }
        if self.show_help {
            // While the help overlay is up, most keys just dismiss it.
            if matches!(key.code, KeyCode::Esc | KeyCode::F(1) | KeyCode::Enter) {
                self.show_help = false;
            }
            return;
        }
        if key.code == KeyCode::F(1) {
            self.show_help = true;
            return;
        }

        match self.screen {
            Screen::ServerSetup => self.server_setup_key(key, ctrl),
            Screen::Accounts => self.accounts_key(key),
            Screen::Login => self.login_key(key, ctrl).await,
            Screen::Chat => self.chat_key(key, ctrl),
            Screen::ConnError => self.conn_error_key(key),
        }
    }

    // --- Server setup -------------------------------------------------------

    fn server_setup_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Esc => match self.setup_return {
                SetupReturn::Start => self.should_quit = true,
                SetupReturn::Reconnect => self.screen = Screen::ConnError,
            },
            KeyCode::Enter => self.submit_server_setup(),
            _ => {
                edit_key(&mut self.server_input, key, ctrl);
            }
        }
    }

    fn submit_server_setup(&mut self) {
        let server = match normalize_server(self.server_input.value()) {
            Some(s) => s,
            None => {
                self.status = "address cannot be empty".into();
                return;
            }
        };
        self.config.server = Some(server.clone());
        session::save_config(&self.config);

        match self.setup_return {
            SetupReturn::Start => {
                if self.store.accounts.is_empty() {
                    self.open_login();
                } else {
                    self.go_to_accounts();
                }
            }
            SetupReturn::Reconnect => {
                // Re-point the pending account at the new address and retry.
                if let Some(p) = self.pending.as_mut() {
                    p.server = server.clone();
                }
                if let Some(acct) = self
                    .store
                    .accounts
                    .iter_mut()
                    .find(|a| Some(&a.username) == self.pending.as_ref().map(|p| &p.username))
                {
                    acct.server = server;
                    session::save(&self.store);
                }
                self.retry();
            }
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

    fn go_to_accounts(&mut self) {
        self.out_tx = None;
        self.authed = false;
        self.screen = Screen::Accounts;
        self.status = "select an account · Enter connect · a add · d delete".into();
    }

    fn open_login(&mut self) {
        self.screen = Screen::Login;
        self.login_mode = LoginMode::Login;
        self.login_field = LoginField::Username;
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

    async fn login_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Esc => {
                if self.store.accounts.is_empty() {
                    self.should_quit = true;
                } else {
                    self.go_to_accounts();
                }
            }
            KeyCode::Tab => {
                self.login_field = match self.login_field {
                    LoginField::Username => LoginField::Password,
                    LoginField::Password => LoginField::Username,
                }
            }
            KeyCode::Char('r') | KeyCode::Char('R') if ctrl => {
                self.login_mode = match self.login_mode {
                    LoginMode::Login => LoginMode::Register,
                    LoginMode::Register => LoginMode::Login,
                }
            }
            KeyCode::Enter => self.submit_login().await,
            _ => {
                let field = match self.login_field {
                    LoginField::Username => &mut self.username_input,
                    LoginField::Password => &mut self.password_input,
                };
                edit_key(field, key, ctrl);
            }
        }
    }

    async fn submit_login(&mut self) {
        let Some(server) = self.config.server.clone() else {
            self.status = "no server configured".into();
            return;
        };
        let path = match self.login_mode {
            LoginMode::Login => "login",
            LoginMode::Register => "register",
        };
        self.status = "connecting…".into();
        match net::auth(&server, path, self.username_input.value(), self.password_input.value()).await {
            Ok(resp) => {
                self.store.upsert(Account {
                    server: server.clone(),
                    username: resp.username.clone(),
                    token: resp.token.clone(),
                });
                session::save(&self.store);
                self.password_input.clear();
                self.connect(server, resp.username, resp.token);
            }
            Err(e) => self.status = format!("auth failed: {e}"),
        }
    }

    // --- Chat screen --------------------------------------------------------

    fn chat_key(&mut self, key: KeyEvent, ctrl: bool) {
        match key.code {
            KeyCode::Esc => self.go_to_accounts(),
            KeyCode::Tab => self.apply_suggestion(),
            KeyCode::Enter => {
                self.submit_chat();
                self.recompute_suggestions();
            }
            _ => {
                if edit_key(&mut self.input, key, ctrl) {
                    self.recompute_suggestions();
                }
            }
        }
    }

    fn submit_chat(&mut self) {
        let line = self.input.value().trim().to_string();
        self.input.clear();
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
                body: line,
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
            Some("accounts") | Some("sessions") => self.go_to_accounts(),
            Some("quit") | Some("q") => self.should_quit = true,
            Some("help") => self.show_help = true,
            _ => self.status = "unknown command (try /help or F1)".into(),
        }
    }

    fn open_chat_with(&mut self, user: String) {
        self.peers.insert(user.clone());
        self.unread.remove(&user);
        self.active_peer = user;
        self.status = format!("now chatting with {}", self.active_peer);
    }

    // --- Connection error screen -------------------------------------------

    fn conn_error_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('r') | KeyCode::Char('R') | KeyCode::Enter => self.retry(),
            KeyCode::Char('s') | KeyCode::Char('S') => {
                self.setup_return = SetupReturn::Reconnect;
                self.server_input = TextInput::with(self.config.server.clone().unwrap_or_default());
                self.screen = Screen::ServerSetup;
                self.status = "enter the new server address".into();
            }
            KeyCode::Char('a') | KeyCode::Char('A') | KeyCode::Esc => self.go_to_accounts(),
            KeyCode::Char('q') | KeyCode::Char('Q') => self.should_quit = true,
            _ => {}
        }
    }

    fn enter_conn_error(&mut self, message: String) {
        self.out_tx = None;
        self.authed = false;
        self.screen = Screen::ConnError;
        self.status = format!("connection failed: {message}");
    }

    fn retry(&mut self) {
        if let Some(p) = self.pending.clone() {
            self.connect(p.server, p.username, p.token);
        } else {
            self.go_to_accounts();
        }
    }

    // --- Autocomplete -------------------------------------------------------

    fn recompute_suggestions(&mut self) {
        self.suggestions.clear();
        self.selected = None; // nothing highlighted until the user presses Tab
        let Some(rest) = self.input.value().strip_prefix('/') else {
            return;
        };
        match rest.split_once(' ') {
            None => {
                for c in COMMANDS {
                    if c.starts_with(rest) {
                        self.suggestions.push(format!("/{c}"));
                    }
                }
            }
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
        let next = match self.selected {
            None => 0,
            Some(i) => (i + 1) % self.suggestions.len(),
        };
        self.selected = Some(next);
        self.input.set(self.suggestions[next].clone());
    }

    // === Mouse handling =====================================================

    pub fn on_mouse(&mut self, ev: MouseEvent) {
        if ev.kind != MouseEventKind::Down(MouseButton::Left) || self.show_help {
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
                if ev.row > list.y && ev.row < list.y + list.height.saturating_sub(1) {
                    let idx = (ev.row - list.y - 1) as usize;
                    if idx < self.store.accounts.len() {
                        self.accounts_idx = idx;
                        self.connect_selected();
                    }
                }
            }
            _ => {}
        }
    }

    // === Incoming events from the network task =============================

    pub fn on_incoming(&mut self, ev: Incoming) {
        match ev {
            Incoming::Server(msg) => self.on_server_msg(msg),
            Incoming::ConnectFailed(m) | Incoming::Disconnected(m) => {
                // Ignore drops that arrive after we already left the chat (e.g.
                // an intentional disconnect, or a token rejection already routed
                // to the login screen).
                if matches!(self.screen, Screen::Chat) && !self.auth_failed {
                    self.enter_conn_error(m);
                }
            }
        }
    }

    fn on_server_msg(&mut self, msg: ServerMsg) {
        match msg {
            ServerMsg::AuthOk { username } => {
                self.authed = true;
                self.status = format!("online as {username} · F1 for help");
            }
            ServerMsg::History { messages } => {
                for m in messages {
                    self.note_peer(&m);
                    self.messages.push(m);
                }
            }
            ServerMsg::Dm(m) => {
                self.note_peer(&m);
                let peer = self.other_party(&m);
                if self.active_peer.is_empty() {
                    self.active_peer = peer.clone();
                    self.unread.remove(&peer);
                } else if m.from != self.username && peer != self.active_peer {
                    *self.unread.entry(peer).or_insert(0) += 1;
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
                if !self.authed && matches!(self.screen, Screen::Chat) {
                    // Rejected before we authenticated — bad/expired token.
                    self.auth_failed = true;
                    self.out_tx = None;
                    self.open_login();
                    self.status = format!("session rejected ({message}) — log in again");
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
        self.pending = Some(PendingConn {
            server: server.clone(),
            username: username.clone(),
            token: token.clone(),
        });
        self.username = username;
        self.server = server.clone();
        self.authed = false;
        self.auth_failed = false;
        self.active_peer.clear();
        self.messages.clear();
        self.peers.clear();
        self.online.clear();
        self.unread.clear();
        self.input.clear();
        self.suggestions.clear();
        self.selected = None;
        self.out_tx = Some(net::spawn_ws(server, token, self.in_tx.clone()));
        self.screen = Screen::Chat;
        self.status = "connecting…".into();
    }
}

/// Apply a text-editing / cursor key to a field. Returns true if the text
/// content changed (so callers can refresh autocomplete).
fn edit_key(field: &mut TextInput, key: KeyEvent, ctrl: bool) -> bool {
    match key.code {
        KeyCode::Char(c) if !ctrl => {
            field.insert(c);
            true
        }
        KeyCode::Backspace => {
            field.backspace();
            true
        }
        KeyCode::Delete => {
            field.delete();
            true
        }
        KeyCode::Left => {
            field.left();
            false
        }
        KeyCode::Right => {
            field.right();
            false
        }
        KeyCode::Home => {
            field.home();
            false
        }
        KeyCode::End => {
            field.end();
            false
        }
        _ => false,
    }
}

/// Trim and ensure the address has a scheme; `None` if empty.
fn normalize_server(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains("://") {
        Some(s.to_string())
    } else {
        Some(format!("http://{s}"))
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
        let config = Config { server: Some("http://x".into()) };
        App::new("http://x".into(), config, Store::default(), tx)
    }

    #[test]
    fn completes_command_names() {
        let mut a = test_app();
        a.input.set("/d");
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
        a.input.set("/dm al");
        a.recompute_suggestions();
        assert_eq!(a.suggestions, vec!["/dm alex", "/dm alice"]);
    }

    #[test]
    fn nothing_highlighted_until_tab() {
        let mut a = test_app();
        a.peers.insert("alice".into());
        a.input.set("/dm al");
        a.recompute_suggestions();
        assert_eq!(a.selected, None); // popup shows, but no selection yet
        a.apply_suggestion();
        assert_eq!(a.selected, Some(0));
        assert_eq!(a.input.value(), "/dm alice");
    }

    #[test]
    fn unread_counts_for_other_chats_only() {
        let mut a = test_app();
        a.username = "me".into();
        a.active_peer = "bob".into();
        // message from alice while talking to bob -> unread for alice
        a.on_server_msg(ServerMsg::Dm(DirectMessage {
            from: "alice".into(),
            to: "me".into(),
            body: "hi".into(),
            ts: 0,
        }));
        assert_eq!(a.unread.get("alice"), Some(&1));
        // message from bob (active) -> no unread
        a.on_server_msg(ServerMsg::Dm(DirectMessage {
            from: "bob".into(),
            to: "me".into(),
            body: "yo".into(),
            ts: 0,
        }));
        assert_eq!(a.unread.get("bob"), None);
        // opening alice clears her unread
        a.open_chat_with("alice".into());
        assert_eq!(a.unread.get("alice"), None);
    }
}
