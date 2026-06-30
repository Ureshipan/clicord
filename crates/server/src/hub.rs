//! In-memory registry of currently connected clients.
//!
//! A single user may be connected from several terminals at once, so each
//! username maps to a set of independent *sessions*. Messages fan out to every
//! session a user has, which keeps multiple devices in sync.

use dashmap::DashMap;
use protocol::ServerMsg;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Default)]
struct UserSessions {
    next_id: u64,
    sessions: HashMap<u64, UnboundedSender<ServerMsg>>,
}

#[derive(Clone, Default)]
pub struct Hub {
    peers: Arc<DashMap<String, UserSessions>>,
}

impl Hub {
    /// Register a new session for `username`.
    ///
    /// Returns the session id and `came_online` — true when this is the user's
    /// first active session (so presence should be broadcast).
    pub fn register(&self, username: &str, tx: UnboundedSender<ServerMsg>) -> (u64, bool) {
        let mut entry = self.peers.entry(username.to_string()).or_default();
        let came_online = entry.sessions.is_empty();
        let id = entry.next_id;
        entry.next_id += 1;
        entry.sessions.insert(id, tx);
        (id, came_online)
    }

    /// Remove a session. Returns true if the user now has no sessions left
    /// (i.e. they just went offline).
    pub fn unregister(&self, username: &str, session_id: u64) -> bool {
        let mut went_offline = false;
        if let Some(mut entry) = self.peers.get_mut(username) {
            entry.sessions.remove(&session_id);
            went_offline = entry.sessions.is_empty();
        }
        // Drop the empty entry (the RefMut above is already out of scope).
        if went_offline {
            self.peers.remove(username);
        }
        went_offline
    }

    /// Deliver a message to every active session of one user.
    pub fn send_to_user(&self, username: &str, msg: ServerMsg) {
        if let Some(entry) = self.peers.get(username) {
            for tx in entry.sessions.values() {
                let _ = tx.send(msg.clone());
            }
        }
    }

    /// Fan a message out to every session of every connected user.
    pub fn broadcast(&self, msg: ServerMsg) {
        for entry in self.peers.iter() {
            for tx in entry.value().sessions.values() {
                let _ = tx.send(msg.clone());
            }
        }
    }

    /// Snapshot of all currently online usernames.
    pub fn online_users(&self) -> Vec<String> {
        self.peers.iter().map(|e| e.key().clone()).collect()
    }
}
