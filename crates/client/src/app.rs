//! Application state and input handling, kept separate from rendering
//! (`ui.rs`), networking (`net.rs`) and persistence (`session.rs`).

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use protocol::{ClientMsg, DirectMessage, GroupId, GroupInfo, GroupMessage, ServerMsg};
use ratatui::layout::Rect;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;

/// How long a "typing" signal stays visible without a refresh.
const TYPING_TTL: Duration = Duration::from_secs(4);
/// Minimum gap between typing signals we emit while the user keeps typing.
const TYPING_THROTTLE: Duration = Duration::from_millis(1500);

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

/// A conversation the user can have open: a direct chat or a group.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Dm(String),
    Group(GroupId),
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

const COMMANDS: &[&str] = &["dm", "group", "g", "find", "accounts", "help", "quit"];

pub struct App {
    pub config: Config,
    pub store: Store,
    pub screen: Screen,
    pub should_quit: bool,
    pub status: String,
    pub show_help: bool,

    setup_return: SetupReturn,

    pub accounts_idx: usize,

    pub server_input: TextInput,

    pub login_mode: LoginMode,
    pub login_field: LoginField,
    pub username_input: TextInput,
    pub password_input: TextInput,

    // chat
    pub username: String,
    pub server: String,
    pub authed: bool,
    pub active: Option<Target>,
    pub input: TextInput,
    pub messages: Vec<DirectMessage>,
    pub group_messages: Vec<GroupMessage>,
    pub groups: BTreeMap<GroupId, GroupInfo>,
    pub peers: BTreeSet<String>,
    pub online: BTreeSet<String>,
    pub directory: BTreeSet<String>,
    pub unread: BTreeMap<Target, u32>,
    /// Messages hidden below the viewport of the active chat. 0 = stuck to the
    /// newest message (autoscroll); > 0 = the user scrolled up.
    pub scroll: usize,
    /// Per-target set of users currently typing, with the last time we heard so.
    pub typing: BTreeMap<Target, BTreeMap<String, Instant>>,
    last_typing_sent: Option<Instant>,
    pending_group: Option<String>,

    // autocomplete
    pub suggestions: Vec<String>,
    pub selected: Option<usize>,

    pending: Option<PendingConn>,
    auth_failed: bool,

    in_tx: UnboundedSender<Incoming>,
    out_tx: Option<UnboundedSender<ClientMsg>>,
}

#[derive(Clone, Copy)]
enum SetupReturn {
    Start,
    Reconnect,
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
            (Screen::Login, "no saved accounts — register or log in".to_string(), TextInput::default())
        } else {
            (Screen::Accounts, "select an account · Enter connect · a add · d delete".to_string(), TextInput::default())
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
            active: None,
            input: TextInput::default(),
            messages: Vec::new(),
            group_messages: Vec::new(),
            groups: BTreeMap::new(),
            peers: BTreeSet::new(),
            online: BTreeSet::new(),
            directory: BTreeSet::new(),
            unread: BTreeMap::new(),
            scroll: 0,
            typing: BTreeMap::new(),
            last_typing_sent: None,
            pending_group: None,
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

        if ctrl && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q')) {
            self.should_quit = true;
            return;
        }
        if self.show_help {
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
                if let Some(p) = self.pending.as_mut() {
                    p.server = server.clone();
                }
                let target_user = self.pending.as_ref().map(|p| p.username.clone());
                if let Some(user) = target_user {
                    if let Some(acct) = self.store.accounts.iter_mut().find(|a| a.username == user) {
                        acct.server = server;
                        session::save(&self.store);
                    }
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
            KeyCode::Up => self.scroll_up(1),
            KeyCode::Down => self.scroll_down(1),
            KeyCode::PageUp => {
                let p = self.msg_viewport().saturating_sub(1).max(1);
                self.scroll_up(p);
            }
            KeyCode::PageDown => {
                let p = self.msg_viewport().saturating_sub(1).max(1);
                self.scroll_down(p);
            }
            KeyCode::Enter => {
                self.submit_chat();
                self.recompute_suggestions();
            }
            _ => {
                if edit_key(&mut self.input, key, ctrl) {
                    self.recompute_suggestions();
                    self.maybe_send_typing();
                }
            }
        }
    }

    // --- Message scrolling --------------------------------------------------

    fn msg_viewport(&self) -> usize {
        let (w, h) = crossterm::terminal::size().unwrap_or((80, 24));
        let m = layout::chat_layout(Rect::new(0, 0, w, h)).messages;
        m.height.saturating_sub(2) as usize
    }

    fn active_msg_count(&self) -> usize {
        match &self.active {
            Some(Target::Dm(p)) => self.dm_messages(p).len(),
            Some(Target::Group(id)) => self.group_messages(*id).len(),
            None => 0,
        }
    }

    fn scroll_up(&mut self, step: usize) {
        let max = self.active_msg_count().saturating_sub(self.msg_viewport().max(1));
        self.scroll = (self.scroll + step).min(max);
    }

    fn scroll_down(&mut self, step: usize) {
        self.scroll = self.scroll.saturating_sub(step);
        if self.scroll == 0 {
            if let Some(t) = self.active.clone() {
                self.unread.remove(&t);
            }
        }
    }

    fn maybe_send_typing(&mut self) {
        let value = self.input.value();
        if value.is_empty() || value.starts_with('/') {
            return;
        }
        let Some(active) = self.active.clone() else { return };
        let now = Instant::now();
        if self
            .last_typing_sent
            .is_some_and(|t| now.duration_since(t) < TYPING_THROTTLE)
        {
            return;
        }
        self.last_typing_sent = Some(now);
        if let Some(tx) = &self.out_tx {
            let msg = match active {
                Target::Dm(p) => ClientMsg::Typing { to_user: Some(p), group_id: None },
                Target::Group(id) => ClientMsg::Typing { to_user: None, group_id: Some(id) },
            };
            let _ = tx.send(msg);
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
        let Some(target) = self.active.clone() else {
            self.status = "no active chat — /dm <user>, /g <group>, or click a name".into();
            return;
        };
        if let Some(tx) = &self.out_tx {
            let msg = match &target {
                Target::Dm(peer) => ClientMsg::SendDm { to: peer.clone(), body: line },
                Target::Group(id) => ClientMsg::SendGroup { group_id: *id, body: line },
            };
            let _ = tx.send(msg);
        }
        // Sending jumps back to the newest message.
        self.scroll = 0;
        self.unread.remove(&target);
    }

    fn handle_command(&mut self, cmd: &str) {
        let mut parts = cmd.split_whitespace();
        match parts.next() {
            Some("dm") | Some("to") => match parts.next() {
                Some(user) => self.open_dm(user.to_string()),
                None => self.status = "usage: /dm <user>".into(),
            },
            Some("group") => {
                let name = parts.next().map(|s| s.to_string());
                let members: Vec<String> = parts.map(|s| s.to_string()).collect();
                match name {
                    Some(name) => {
                        self.pending_group = Some(name.clone());
                        if let Some(tx) = &self.out_tx {
                            let _ = tx.send(ClientMsg::CreateGroup { name: name.clone(), members });
                        }
                        self.status = format!("creating group #{name}…");
                    }
                    None => self.status = "usage: /group <name> [members...]".into(),
                }
            }
            Some("g") | Some("open") => match parts.next() {
                Some(name) => self.open_group_by_name(name),
                None => self.status = "usage: /g <group-name>".into(),
            },
            Some("find") | Some("search") => {
                let query = parts.collect::<Vec<_>>().join(" ");
                if query.is_empty() {
                    self.status = "usage: /find <prefix>".into();
                } else if let Some(tx) = &self.out_tx {
                    let _ = tx.send(ClientMsg::SearchUsers { query });
                    self.status = "searching…".into();
                }
            }
            Some("accounts") | Some("sessions") => self.go_to_accounts(),
            Some("quit") | Some("q") => self.should_quit = true,
            Some("help") => self.show_help = true,
            _ => self.status = "unknown command (try /help or F1)".into(),
        }
    }

    fn open_dm(&mut self, user: String) {
        self.peers.insert(user.clone());
        let t = Target::Dm(user.clone());
        self.unread.remove(&t);
        self.active = Some(t);
        self.scroll = 0;
        self.status = format!("now chatting with {user}");
    }

    fn open_group(&mut self, id: GroupId) {
        let t = Target::Group(id);
        self.unread.remove(&t);
        let name = self.groups.get(&id).map(|g| g.name.clone()).unwrap_or_default();
        self.active = Some(t);
        self.scroll = 0;
        self.status = format!("now in group #{name}");
    }

    fn open_group_by_name(&mut self, name: &str) {
        match self.groups.values().find(|g| g.name == name).map(|g| g.id) {
            Some(id) => self.open_group(id),
            None => self.status = format!("no group named #{name}"),
        }
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
        self.selected = None;
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
            Some((cmd, arg)) if cmd == "g" || cmd == "open" => {
                for g in self.groups.values() {
                    if g.name.starts_with(arg) {
                        self.suggestions.push(format!("/{cmd} {}", g.name));
                    }
                }
            }
            // For /group, complete the last whitespace-separated token as a user.
            Some(("group", arg)) => {
                if let Some((head, last)) = arg.rsplit_once(' ') {
                    for u in self.known_users() {
                        if u != self.username && u.starts_with(last) {
                            self.suggestions.push(format!("/group {head} {u}"));
                        }
                    }
                }
            }
            Some(_) => {}
        }
    }

    fn known_users(&self) -> BTreeSet<String> {
        self.peers
            .union(&self.online)
            .chain(self.directory.iter())
            .cloned()
            .collect()
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
        if self.show_help {
            return;
        }
        // Mouse wheel scrolls the message history.
        if matches!(self.screen, Screen::Chat) {
            match ev.kind {
                MouseEventKind::ScrollUp => return self.scroll_up(3),
                MouseEventKind::ScrollDown => return self.scroll_down(3),
                _ => {}
            }
        }
        if ev.kind != MouseEventKind::Down(MouseButton::Left) {
            return;
        }
        let (w, h) = crossterm::terminal::size().unwrap_or((80, 24));
        let area = Rect::new(0, 0, w, h);
        match self.screen {
            Screen::Chat => {
                let panel = layout::chat_layout(area).peers;
                if let Some(t) = self.entry_at(panel, ev.column, ev.row) {
                    match t {
                        Target::Dm(u) => self.open_dm(u),
                        Target::Group(id) => self.open_group(id),
                    }
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

    fn entry_at(&self, rect: Rect, col: u16, row: u16) -> Option<Target> {
        let inside_x = col >= rect.x && col < rect.x + rect.width;
        let inside_y = row > rect.y && row < rect.y + rect.height.saturating_sub(1);
        if !inside_x || !inside_y {
            return None;
        }
        let idx = (row - rect.y - 1) as usize;
        self.chat_entries().into_iter().nth(idx)
    }

    // === Incoming events ===================================================

    pub fn on_incoming(&mut self, ev: Incoming) {
        match ev {
            Incoming::Server(msg) => self.on_server_msg(msg),
            Incoming::ConnectFailed(m) | Incoming::Disconnected(m) => {
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
                let from_self = m.from == self.username;
                let t = Target::Dm(self.other_party(&m));
                self.messages.push(m);
                if self.active.is_none() {
                    self.active = Some(t);
                    self.scroll = 0;
                } else {
                    self.bump_for_incoming(t, from_self);
                }
            }
            ServerMsg::Groups { groups } => {
                for g in groups {
                    self.groups.insert(g.id, g);
                }
            }
            ServerMsg::GroupCreated(info) => {
                let id = info.id;
                let name = info.name.clone();
                self.groups.insert(id, info);
                if self.pending_group.as_deref() == Some(name.as_str()) {
                    self.pending_group = None;
                    self.open_group(id);
                } else {
                    self.status = format!("added to group #{name}");
                }
            }
            ServerMsg::GroupMsg(gm) => {
                let from_self = gm.from == self.username;
                let t = Target::Group(gm.group_id);
                self.group_messages.push(gm);
                self.bump_for_incoming(t, from_self);
            }
            ServerMsg::GroupHistory { messages, .. } => {
                self.group_messages.extend(messages);
            }
            ServerMsg::Typing { from, group_id } => {
                let t = match group_id {
                    Some(id) => Target::Group(id),
                    None => Target::Dm(from.clone()),
                };
                self.typing.entry(t).or_default().insert(from, Instant::now());
            }
            ServerMsg::SearchResults { query, users } => {
                for u in &users {
                    self.directory.insert(u.clone());
                }
                self.status = if users.is_empty() {
                    format!("no users match '{query}'")
                } else {
                    format!("found: {}", users.join(", "))
                };
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

    /// React to a freshly-appended message for conversation `t`.
    fn bump_for_incoming(&mut self, t: Target, from_self: bool) {
        if self.active.as_ref() == Some(&t) {
            // Active chat: if the user scrolled up, keep the view anchored on
            // the same messages and surface an unread count instead of moving.
            if self.scroll > 0 {
                self.scroll += 1;
                if !from_self {
                    *self.unread.entry(t).or_insert(0) += 1;
                }
            }
        } else if !from_self {
            *self.unread.entry(t).or_insert(0) += 1;
        }
    }

    /// Periodic housekeeping: drop stale "typing" signals.
    pub fn tick(&mut self) {
        let now = Instant::now();
        for users in self.typing.values_mut() {
            users.retain(|_, t| now.duration_since(*t) < TYPING_TTL);
        }
        self.typing.retain(|_, users| !users.is_empty());
    }

    /// "alice is typing…" for the active conversation, if anyone is.
    pub fn typing_text(&self) -> Option<String> {
        let t = self.active.as_ref()?;
        let now = Instant::now();
        let names: Vec<&str> = self
            .typing
            .get(t)?
            .iter()
            .filter(|(u, seen)| **u != self.username && now.duration_since(**seen) < TYPING_TTL)
            .map(|(u, _)| u.as_str())
            .collect();
        if names.is_empty() {
            return None;
        }
        let joined = names.join(", ");
        Some(if names.len() == 1 {
            format!("{joined} is typing…")
        } else {
            format!("{joined} are typing…")
        })
    }

    // === Views used by the renderer ========================================

    /// All open conversations, groups first then DM peers — the order the
    /// chat list and mouse hit-testing both rely on.
    pub fn chat_entries(&self) -> Vec<Target> {
        let mut v: Vec<Target> = self.groups.keys().map(|id| Target::Group(*id)).collect();
        v.extend(self.peers.iter().cloned().map(Target::Dm));
        v
    }

    pub fn target_name(&self, t: &Target) -> String {
        match t {
            Target::Dm(u) => u.clone(),
            Target::Group(id) => self
                .groups
                .get(id)
                .map(|g| format!("#{}", g.name))
                .unwrap_or_else(|| format!("#{id}")),
        }
    }

    pub fn active_name(&self) -> String {
        match &self.active {
            None => "(no chat)".into(),
            Some(t) => self.target_name(t),
        }
    }

    pub fn dm_messages(&self, peer: &str) -> Vec<&DirectMessage> {
        self.messages
            .iter()
            .filter(|m| {
                (m.from == self.username && m.to == peer) || (m.from == peer && m.to == self.username)
            })
            .collect()
    }

    pub fn group_messages(&self, id: GroupId) -> Vec<&GroupMessage> {
        self.group_messages.iter().filter(|m| m.group_id == id).collect()
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
        self.active = None;
        self.messages.clear();
        self.group_messages.clear();
        self.groups.clear();
        self.peers.clear();
        self.online.clear();
        self.directory.clear();
        self.unread.clear();
        self.scroll = 0;
        self.typing.clear();
        self.last_typing_sent = None;
        self.pending_group = None;
        self.input.clear();
        self.suggestions.clear();
        self.selected = None;
        self.out_tx = Some(net::spawn_ws(server, token, self.in_tx.clone()));
        self.screen = Screen::Chat;
        self.status = "connecting…".into();
    }
}

/// Apply a text-editing / cursor key to a field. Returns true if the content
/// changed (so callers can refresh autocomplete).
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
        a.input.set("/gr");
        a.recompute_suggestions();
        assert_eq!(a.suggestions, vec!["/group"]);
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
        assert_eq!(a.selected, None);
        a.apply_suggestion();
        assert_eq!(a.selected, Some(0));
        assert_eq!(a.input.value(), "/dm alice");
    }

    #[test]
    fn unread_counts_for_inactive_targets() {
        let mut a = test_app();
        a.username = "me".into();
        a.active = Some(Target::Dm("bob".into()));
        a.on_server_msg(ServerMsg::Dm(DirectMessage {
            from: "alice".into(),
            to: "me".into(),
            body: "hi".into(),
            ts: 0,
        }));
        assert_eq!(a.unread.get(&Target::Dm("alice".into())), Some(&1));
        a.open_dm("alice".into());
        assert_eq!(a.unread.get(&Target::Dm("alice".into())), None);
    }

    #[test]
    fn group_created_when_pending_opens_it() {
        let mut a = test_app();
        a.username = "me".into();
        a.pending_group = Some("team".into());
        a.on_server_msg(ServerMsg::GroupCreated(GroupInfo {
            id: 7,
            name: "team".into(),
            members: vec!["me".into(), "bob".into()],
        }));
        assert_eq!(a.active, Some(Target::Group(7)));
        assert!(a.groups.contains_key(&7));
    }

    #[test]
    fn scrolled_up_active_chat_anchors_and_counts() {
        let mut a = test_app();
        a.username = "me".into();
        a.active = Some(Target::Dm("bob".into()));
        a.scroll = 3;
        a.on_server_msg(ServerMsg::Dm(DirectMessage {
            from: "bob".into(),
            to: "me".into(),
            body: "x".into(),
            ts: 0,
        }));
        // View anchored (scroll grew with the message) and unread counted.
        assert_eq!(a.scroll, 4);
        assert_eq!(a.unread.get(&Target::Dm("bob".into())), Some(&1));
    }

    #[test]
    fn at_bottom_active_chat_stays_read() {
        let mut a = test_app();
        a.username = "me".into();
        a.active = Some(Target::Dm("bob".into()));
        a.scroll = 0;
        a.on_server_msg(ServerMsg::Dm(DirectMessage {
            from: "bob".into(),
            to: "me".into(),
            body: "x".into(),
            ts: 0,
        }));
        assert_eq!(a.scroll, 0);
        assert_eq!(a.unread.get(&Target::Dm("bob".into())), None);
    }

    #[test]
    fn typing_text_reports_active_typists() {
        let mut a = test_app();
        a.username = "me".into();
        a.active = Some(Target::Dm("bob".into()));
        a.on_server_msg(ServerMsg::Typing { from: "bob".into(), group_id: None });
        assert_eq!(a.typing_text().as_deref(), Some("bob is typing…"));
    }

    #[test]
    fn group_unread_when_not_active() {
        let mut a = test_app();
        a.username = "me".into();
        a.groups.insert(3, GroupInfo { id: 3, name: "g".into(), members: vec![] });
        a.active = Some(Target::Dm("bob".into()));
        a.on_server_msg(ServerMsg::GroupMsg(GroupMessage {
            group_id: 3,
            from: "alice".into(),
            body: "yo".into(),
            ts: 0,
        }));
        assert_eq!(a.unread.get(&Target::Group(3)), Some(&1));
    }
}
