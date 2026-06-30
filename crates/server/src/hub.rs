//! In-memory registry of currently connected clients, used to route messages
//! between online users. Persistence lives in the database; this is purely the
//! live fan-out layer.

use dashmap::DashMap;
use protocol::ServerMsg;
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Clone, Default)]
pub struct Hub {
    peers: Arc<DashMap<String, UnboundedSender<ServerMsg>>>,
}

impl Hub {
    pub fn register(&self, username: &str, tx: UnboundedSender<ServerMsg>) {
        self.peers.insert(username.to_string(), tx);
    }

    pub fn unregister(&self, username: &str) {
        self.peers.remove(username);
    }

    /// Deliver to a single user if they are online. Returns whether delivered.
    pub fn send_to(&self, username: &str, msg: ServerMsg) -> bool {
        match self.peers.get(username) {
            Some(tx) => tx.send(msg).is_ok(),
            None => false,
        }
    }

    /// Fan a message out to every connected client.
    pub fn broadcast(&self, msg: ServerMsg) {
        for entry in self.peers.iter() {
            let _ = entry.value().send(msg.clone());
        }
    }
}
