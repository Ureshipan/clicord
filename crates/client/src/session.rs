//! Local, on-disk store of logged-in accounts so the client can offer an
//! account picker on startup and reconnect with a stored token.
//!
//! Path is platform-native via `dirs::config_dir()`:
//!   Linux:   ~/.config/clicord/sessions.json
//!   Windows: %APPDATA%\clicord\sessions.json
//!   macOS:   ~/Library/Application Support/clicord/sessions.json
//!
//! Tokens are stored in plaintext — fine for a desktop app, but worth
//! revisiting (OS keyring) before treating this as hardened.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub server: String,
    pub username: String,
    pub token: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    pub accounts: Vec<Account>,
}

/// Application-level config, separate from the account store. Holds the server
/// address entered on first run.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    pub server: Option<String>,
}

impl Store {
    /// Insert or replace the account matched by (server, username).
    pub fn upsert(&mut self, account: Account) {
        if let Some(existing) = self
            .accounts
            .iter_mut()
            .find(|a| a.server == account.server && a.username == account.username)
        {
            existing.token = account.token;
        } else {
            self.accounts.push(account);
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.accounts.len() {
            self.accounts.remove(index);
        }
    }
}

fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("clicord"))
}

fn store_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("sessions.json"))
}

fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.json"))
}

/// Load the app config; returns an empty config if missing or unreadable.
pub fn load_config() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Config::default(),
    }
}

pub fn save_config(config: &Config) {
    let Some(path) = config_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(config) {
        let _ = std::fs::write(&path, text);
    }
}

/// Load the account store; returns an empty store if missing or unreadable.
pub fn load() -> Store {
    let Some(path) = store_path() else {
        return Store::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
        Err(_) => Store::default(),
    }
}

/// Persist the account store, creating the config directory if needed.
pub fn save(store: &Store) {
    let Some(path) = store_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(store) {
        let _ = std::fs::write(&path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acct(token: &str) -> Account {
        Account {
            server: "s".into(),
            username: "u".into(),
            token: token.into(),
        }
    }

    #[test]
    fn upsert_replaces_token_for_same_identity() {
        let mut s = Store::default();
        s.upsert(acct("t1"));
        s.upsert(acct("t2"));
        assert_eq!(s.accounts.len(), 1);
        assert_eq!(s.accounts[0].token, "t2");
    }

    #[test]
    fn upsert_distinguishes_username_and_server() {
        let mut s = Store::default();
        s.upsert(acct("t1"));
        s.upsert(Account { username: "other".into(), ..acct("t1") });
        assert_eq!(s.accounts.len(), 2);
    }

    #[test]
    fn remove_is_bounds_safe() {
        let mut s = Store::default();
        s.upsert(acct("t1"));
        s.remove(5); // out of range — no panic
        assert_eq!(s.accounts.len(), 1);
        s.remove(0);
        assert!(s.accounts.is_empty());
    }
}
