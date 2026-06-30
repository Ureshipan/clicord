//! Shared wire protocol between the clicord client and server.
//!
//! Keeping these types in a dedicated crate means the contract can never
//! silently drift between the two sides — both depend on the same definitions.

use serde::{Deserialize, Serialize};

/// For now a user is identified by their (unique) username.
pub type UserId = String;

/// Body of the `POST /api/register` and `POST /api/login` HTTP endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthRequest {
    pub username: String,
    pub password: String,
}

/// Successful response of the auth HTTP endpoints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    /// Bearer token used to authenticate the websocket connection.
    pub token: String,
    pub username: String,
}

/// A single direct message as stored and transported.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirectMessage {
    pub from: String,
    pub to: String,
    pub body: String,
    /// Unix timestamp in milliseconds.
    pub ts: i64,
}

/// Messages the client sends to the server over the websocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    /// MUST be the first frame after the socket opens.
    Auth { token: String },
    /// Send a direct message to `to`.
    SendDm { to: String, body: String },
    /// Keepalive.
    Ping,
}

/// Messages the server sends to the client over the websocket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    /// Authentication of the socket succeeded.
    AuthOk { username: String },
    /// A direct message involving this client (sent or received).
    Dm(DirectMessage),
    /// Recent history replayed right after auth.
    History { messages: Vec<DirectMessage> },
    /// Presence change of some user.
    Presence { username: String, online: bool },
    /// A recoverable, human-readable error.
    Error { message: String },
    /// Keepalive answer.
    Pong,
}
