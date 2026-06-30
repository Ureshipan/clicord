//! Websocket endpoint: authenticates the socket, then routes direct messages
//! between online users while persisting everything to the database.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use protocol::{ClientMsg, DirectMessage, ServerMsg};
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
    st.hub.register(&username, tx.clone());

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

    // Replay recent history so the client opens with context.
    if let Ok(history) = db::recent_for_user(&st.db, &username, 100).await {
        let _ = tx.send(ServerMsg::History { messages: history });
    }

    // Tell everyone this user just came online.
    st.hub.broadcast(ServerMsg::Presence { username: username.clone(), online: true });

    tracing::info!(%username, "client connected");

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
    st.hub.unregister(&username);
    st.hub.broadcast(ServerMsg::Presence { username: username.clone(), online: false });
    tracing::info!(%username, "client disconnected");
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

            // Deliver to the recipient if online, and echo back to the sender
            // so their own UI shows the sent message.
            st.hub.send_to(&to, ServerMsg::Dm(dm.clone()));
            let _ = self_tx.send(ServerMsg::Dm(dm));
        }
    }
    true
}
