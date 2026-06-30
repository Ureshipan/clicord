//! Application state and input handling, kept deliberately separate from
//! rendering (`ui.rs`) and networking (`net.rs`) so screens can be added or
//! reworked without touching the other layers.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use protocol::{ClientMsg, DirectMessage, ServerMsg};
use std::collections::BTreeSet;
use tokio::sync::mpsc::UnboundedSender;

use crate::net;

pub enum Screen {
    Login,
    Chat,
}

#[derive(Clone, Copy)]
pub enum LoginMode {
    Login,
    Register,
}

#[derive(Clone, Copy)]
pub enum LoginField {
    Username,
    Password,
}

pub struct App {
    pub server: String,
    pub screen: Screen,
    pub should_quit: bool,
    pub status: String,

    // login screen
    pub login_mode: LoginMode,
    pub login_field: LoginField,
    pub username_input: String,
    pub password_input: String,

    // chat screen
    pub username: String,
    pub active_peer: String,
    pub input: String,
    pub messages: Vec<DirectMessage>,
    pub peers: BTreeSet<String>,
    pub online: BTreeSet<String>,

    // networking
    in_tx: UnboundedSender<ServerMsg>,
    out_tx: Option<UnboundedSender<ClientMsg>>,
}

impl App {
    pub fn new(server: String, in_tx: UnboundedSender<ServerMsg>) -> Self {
        Self {
            server,
            screen: Screen::Login,
            should_quit: false,
            status: "enter credentials — Ctrl+R toggles login/register".into(),
            login_mode: LoginMode::Login,
            login_field: LoginField::Username,
            username_input: String::new(),
            password_input: String::new(),
            username: String::new(),
            active_peer: String::new(),
            input: String::new(),
            messages: Vec::new(),
            peers: BTreeSet::new(),
            online: BTreeSet::new(),
            in_tx,
            out_tx: None,
        }
    }

    pub async fn on_key(&mut self, key: KeyEvent) {
        match self.screen {
            Screen::Login => self.login_key(key).await,
            Screen::Chat => self.chat_key(key),
        }
    }

    // --- Login --------------------------------------------------------------

    async fn login_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.should_quit = true,
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
            KeyCode::Backspace => match self.login_field {
                LoginField::Username => {
                    self.username_input.pop();
                }
                LoginField::Password => {
                    self.password_input.pop();
                }
            },
            KeyCode::Char(c) if !ctrl => match self.login_field {
                LoginField::Username => self.username_input.push(c),
                LoginField::Password => self.password_input.push(c),
            },
            _ => {}
        }
    }

    async fn submit_login(&mut self) {
        let path = match self.login_mode {
            LoginMode::Login => "login",
            LoginMode::Register => "register",
        };
        self.status = "connecting…".into();
        match net::auth(&self.server, path, &self.username_input, &self.password_input).await {
            Ok(resp) => {
                self.username = resp.username;
                self.out_tx = Some(net::spawn_ws(self.server.clone(), resp.token, self.in_tx.clone()));
                self.screen = Screen::Chat;
                self.status = "connected — use /dm <user> to start a chat".into();
                self.password_input.clear();
            }
            Err(e) => self.status = format!("auth failed: {e}"),
        }
    }

    // --- Chat ---------------------------------------------------------------

    fn chat_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => self.should_quit = true,
            KeyCode::Enter => self.submit_chat(),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) if !ctrl => self.input.push(c),
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
            self.status = "no active chat — use /dm <user> first".into();
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
                Some(user) => {
                    self.active_peer = user.to_string();
                    self.peers.insert(user.to_string());
                    self.status = format!("now chatting with {user}");
                }
                None => self.status = "usage: /dm <user>".into(),
            },
            Some("quit") | Some("q") => self.should_quit = true,
            Some("help") => self.status = "/dm <user> — open a chat   /quit — exit".into(),
            _ => self.status = "unknown command (try /help)".into(),
        }
    }

    // --- Incoming server events --------------------------------------------

    pub fn on_server_msg(&mut self, msg: ServerMsg) {
        match msg {
            ServerMsg::AuthOk { username } => self.status = format!("authenticated as {username}"),
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
            ServerMsg::Error { message } => self.status = format!("server: {message}"),
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
}
