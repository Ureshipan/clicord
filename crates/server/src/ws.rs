//! Websocket endpoint: authenticates the socket, then routes direct messages
//! between online users while persisting everything to the database.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMsg, DirectMessage, GroupMessage, ServerMsg};
use tokio::sync::mpsc;

use crate::{auth, db, AppState};

pub async fn ws_handler(ws: WebSocketUpgrade, State(st): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, st))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

async fn handle_socket(socket: WebSocket, st: AppState) {
    let (mut ws_tx, mut ws_rx) = socket.split();

    // --- Authentication: the first frame must be ClientMsg::Auth -------------
    let username = match authenticate(&mut ws_rx, &st).await {
        Some(u) => u,
        None => {
            let _ = ws_tx
                .send(Message::Text(
                    serde_json::to_string(&ServerMsg::Error {
                        message: "authentication required".into(),
                    })
                    .unwrap(),
                ))
                .await;
            return;
        }
    };

    // --- Wire up the outgoing pump ------------------------------------------
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerMsg>();
    let (session_id, came_online) = st.hub.register(&username, tx.clone());

    let mut send_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            let txt = match serde_json::to_string(&msg) {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ws_tx.send(Message::Text(txt)).await.is_err() {
                break;
            }
        }
    });

    let _ = tx.send(ServerMsg::AuthOk { username: username.clone() });

    // Replay recent DM history so the client opens with context.
    if let Ok(history) = db::recent_for_user(&st.db, &username, 100).await {
        let _ = tx.send(ServerMsg::History { messages: history });
    }

    // Send the user's groups and recent group history.
    if let Ok(groups) = db::groups_for_user(&st.db, &username).await {
        for g in &groups {
            if let Ok(messages) = db::recent_group_messages(&st.db, g.id, 100).await {
                let _ = tx.send(ServerMsg::GroupHistory { group_id: g.id, messages });
            }
        }
        let _ = tx.send(ServerMsg::Groups { groups });
    }

    // Send this client a snapshot of who is currently online.
    for other in st.hub.online_users() {
        if other != username {
            let _ = tx.send(ServerMsg::Presence { username: other, online: true });
        }
    }

    // Announce presence only when this is the user's first session.
    if came_online {
        st.hub.broadcast(ServerMsg::Presence { username: username.clone(), online: true });
    }

    tracing::info!(%username, session_id, "session connected");

    // --- Read loop -----------------------------------------------------------
    loop {
        tokio::select! {
            // If the outgoing pump dies (socket closed on write side), stop.
            _ = &mut send_task => break,
            incoming = ws_rx.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if !handle_client_msg(&st, &username, &tx, &text).await {
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {} // ignore binary/ping/pong frames
                    Some(Err(_)) => break,
                }
            }
        }
    }

    // --- Cleanup -------------------------------------------------------------
    send_task.abort();
    let went_offline = st.hub.unregister(&username, session_id);
    if went_offline {
        st.hub.broadcast(ServerMsg::Presence { username: username.clone(), online: false });
    }
    tracing::info!(%username, session_id, "session disconnected");
}

/// Wait for the first valid Auth frame. Returns the authenticated username.
async fn authenticate(
    ws_rx: &mut futures_util::stream::SplitStream<WebSocket>,
    st: &AppState,
) -> Option<String> {
    while let Some(Ok(msg)) = ws_rx.next().await {
        if let Message::Text(text) = msg {
            if let Ok(ClientMsg::Auth { token }) = serde_json::from_str::<ClientMsg>(&text) {
                if let Ok(username) = auth::verify_token(&st.jwt_secret, &token) {
                    return Some(username);
                }
                return None;
            }
        }
    }
    None
}

/// Handle a single client frame. Returns false if the connection should close.
async fn handle_client_msg(
    st: &AppState,
    username: &str,
    self_tx: &mpsc::UnboundedSender<ServerMsg>,
    text: &str,
) -> bool {
    let msg = match serde_json::from_str::<ClientMsg>(text) {
        Ok(m) => m,
        Err(_) => {
            let _ = self_tx.send(ServerMsg::Error { message: "malformed frame".into() });
            return true;
        }
    };

    match msg {
        ClientMsg::Auth { .. } => {} // already authenticated; ignore re-auth
        ClientMsg::Ping => {
            let _ = self_tx.send(ServerMsg::Pong);
        }
        ClientMsg::SendDm { to, body } => {
            if body.trim().is_empty() {
                return true;
            }
            let dm = DirectMessage {
                from: username.to_string(),
                to: to.clone(),
                body,
                ts: now_ms(),
            };

            if let Err(e) = db::store_message(&st.db, &dm).await {
                tracing::warn!(error = %e, "failed to persist message");
                let _ = self_tx.send(ServerMsg::Error { message: "failed to store message".into() });
                return true;
            }

            // Deliver to every session of the recipient, and echo to every
            // session of the sender (so all their terminals stay in sync).
            st.hub.send_to_user(&to, ServerMsg::Dm(dm.clone()));
            if to != username {
                st.hub.send_to_user(username, ServerMsg::Dm(dm));
            }
        }
        ClientMsg::CreateGroup { name, mut members } => {
            let name = name.trim().to_string();
            if name.is_empty() {
                let _ = self_tx.send(ServerMsg::Error { message: "group name required".into() });
                return true;
            }
            // The creator is always a member.
            members.push(username.to_string());
            members.sort();
            members.dedup();

            match db::create_group(&st.db, &name, &members).await {
                Ok(info) => {
                    // Notify every member (all their sessions).
                    for m in &info.members {
                        st.hub.send_to_user(m, ServerMsg::GroupCreated(info.clone()));
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "failed to create group");
                    let _ = self_tx.send(ServerMsg::Error { message: "failed to create group".into() });
                }
            }
        }
        ClientMsg::SendGroup { group_id, body } => {
            if body.trim().is_empty() {
                return true;
            }
            match db::is_member(&st.db, group_id, username).await {
                Ok(true) => {}
                _ => {
                    let _ = self_tx.send(ServerMsg::Error { message: "not a member of that group".into() });
                    return true;
                }
            }
            let gm = GroupMessage {
                group_id,
                from: username.to_string(),
                body,
                ts: now_ms(),
            };
            if let Err(e) = db::store_group_message(&st.db, &gm).await {
                tracing::warn!(error = %e, "failed to persist group message");
                let _ = self_tx.send(ServerMsg::Error { message: "failed to store message".into() });
                return true;
            }
            // Fan out to every member's sessions (includes the sender).
            match db::group_members(&st.db, group_id).await {
                Ok(members) => {
                    for m in members {
                        st.hub.send_to_user(&m, ServerMsg::GroupMsg(gm.clone()));
                    }
                }
                Err(e) => tracing::warn!(error = %e, "failed to load group members"),
            }
        }
        ClientMsg::SearchUsers { query } => {
            let query = query.trim().to_string();
            let users = if query.is_empty() {
                Vec::new()
            } else {
                db::search_users(&st.db, &query, 20).await.unwrap_or_default()
            };
            let _ = self_tx.send(ServerMsg::SearchResults { query, users });
        }
        ClientMsg::Typing { to_user, group_id } => {
            match (group_id, to_user) {
                (Some(gid), _) => {
                    if let Ok(members) = db::group_members(&st.db, gid).await {
                        for m in members {
                            if m != username {
                                st.hub.send_to_user(
                                    &m,
                                    ServerMsg::Typing { from: username.to_string(), group_id: Some(gid) },
                                );
                            }
                        }
                    }
                }
                (None, Some(to)) if to != username => {
                    st.hub.send_to_user(
                        &to,
                        ServerMsg::Typing { from: username.to_string(), group_id: None },
                    );
                }
                _ => {}
            }
        }
    }
    true
}
