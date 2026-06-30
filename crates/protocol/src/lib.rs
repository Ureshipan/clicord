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

pub type GroupId = i64;

/// Metadata about a group chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupInfo {
    pub id: GroupId,
    pub name: String,
    pub members: Vec<String>,
}

/// A single message sent to a group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupMessage {
    pub group_id: GroupId,
    pub from: String,
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
    /// Create a group with the given name and initial members (besides the
    /// creator, who is always added).
    CreateGroup { name: String, members: Vec<String> },
    /// Send a message to a group the sender belongs to.
    SendGroup { group_id: GroupId, body: String },
    /// Search registered users whose name starts with `query`.
    SearchUsers { query: String },
    /// Signal that the sender is typing in a DM (`to_user`) or a group
    /// (`group_id`). Exactly one of the two is expected to be set.
    Typing { to_user: Option<String>, group_id: Option<GroupId> },
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
    /// Recent DM history replayed right after auth.
    History { messages: Vec<DirectMessage> },
    /// The groups this user belongs to (sent on connect, and on changes).
    Groups { groups: Vec<GroupInfo> },
    /// A group this user was just added to / created.
    GroupCreated(GroupInfo),
    /// A message in one of this user's groups.
    GroupMsg(GroupMessage),
    /// Recent history for a single group.
    GroupHistory { group_id: GroupId, messages: Vec<GroupMessage> },
    /// Results of a user search.
    SearchResults { query: String, users: Vec<String> },
    /// `from` is typing. `group_id` set => in that group; otherwise a DM from
    /// `from` to this client.
    Typing { from: String, group_id: Option<GroupId> },
    /// Presence change of some user.
    Presence { username: String, online: bool },
    /// A recoverable, human-readable error.
    Error { message: String },
    /// Keepalive answer.
    Pong,
}
