//! Networking for the client: HTTP auth + the background websocket task.
//!
//! The websocket runs in its own tokio task and talks to the UI through two
//! channels — outgoing `ClientMsg`s in, incoming `ServerMsg`s out — so the
//! render loop never blocks on the network.

use anyhow::{anyhow, Result};
use futures_util::{SinkExt, StreamExt};
use protocol::{Attachment, AuthRequest, AuthResponse, ClientMsg, ServerMsg};
use std::path::Path;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::media::{self, ImageArt};

/// Events the websocket task delivers to the UI. Separating transport-level
/// outcomes from protocol frames lets the UI react to drops and failures.
pub enum Incoming {
    /// A decoded protocol frame from the server.
    Server(ServerMsg),
    /// The connection could not be established at all.
    ConnectFailed(String),
    /// An established connection was lost.
    Disconnected(String),
    /// A one-off status line from a background task (upload/download result).
    Notice(String),
    /// A decoded inline thumbnail for an image attachment.
    Preview { id: i64, art: ImageArt },
}

/// Perform `POST /api/{login,register}` and return the issued token.
pub async fn auth(server: &str, path: &str, username: &str, password: &str) -> Result<AuthResponse> {
    let url = format!("{}/api/{}", server.trim_end_matches('/'), path);
    let resp = reqwest::Client::new()
        .post(&url)
        .json(&AuthRequest {
            username: username.to_string(),
            password: password.to_string(),
        })
        .send()
        .await?;

    if !resp.status().is_success() {
        let code = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("{code}: {body}"));
    }
    Ok(resp.json::<AuthResponse>().await?)
}

/// Upload a local file as an attachment and return its stored metadata.
pub async fn upload(server: &str, token: &str, path: &Path) -> Result<Attachment> {
    let bytes = tokio::fs::read(path).await.map_err(|e| anyhow!("cannot read file: {e}"))?;
    if bytes.is_empty() {
        return Err(anyhow!("file is empty"));
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    let mime = media::guess_mime(path);
    let url = format!("{}/api/upload", server.trim_end_matches('/'));

    let resp = reqwest::Client::new()
        .post(&url)
        .bearer_auth(token)
        .query(&[("name", name.as_str()), ("mime", mime.as_str())])
        .body(bytes)
        .send()
        .await?;

    if !resp.status().is_success() {
        let code = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("{code}: {body}"));
    }
    Ok(resp.json::<Attachment>().await?)
}

/// Download an attachment's bytes by id.
pub async fn download(server: &str, token: &str, id: i64) -> Result<Vec<u8>> {
    let url = format!("{}/api/attachment/{}", server.trim_end_matches('/'), id);
    let resp = reqwest::Client::new().get(&url).bearer_auth(token).send().await?;
    if !resp.status().is_success() {
        let code = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow!("{code}: {body}"));
    }
    Ok(resp.bytes().await?.to_vec())
}

/// Spawn the websocket task. Returns a channel for sending `ClientMsg`s; the
/// task forwards transport events and server frames to `in_tx`.
pub fn spawn_ws(
    server: String,
    token: String,
    in_tx: UnboundedSender<Incoming>,
) -> UnboundedSender<ClientMsg> {
    let (out_tx, mut out_rx) = mpsc::unbounded_channel::<ClientMsg>();

    tokio::spawn(async move {
        let ws_url = http_to_ws(&server);
        let (ws_stream, _) = match connect_async(ws_url.as_str()).await {
            Ok(s) => s,
            Err(e) => {
                let _ = in_tx.send(Incoming::ConnectFailed(format!("{e}")));
                return;
            }
        };
        let (mut write, mut read) = ws_stream.split();

        // Authenticate the socket immediately.
        let auth_frame = serde_json::to_string(&ClientMsg::Auth { token }).unwrap();
        if write.send(Message::Text(auth_frame)).await.is_err() {
            let _ = in_tx.send(Incoming::ConnectFailed("failed to send auth".into()));
            return;
        }

        loop {
            tokio::select! {
                outgoing = out_rx.recv() => match outgoing {
                    Some(msg) => {
                        let txt = serde_json::to_string(&msg).unwrap();
                        if write.send(Message::Text(txt)).await.is_err() {
                            let _ = in_tx.send(Incoming::Disconnected("send failed".into()));
                            break;
                        }
                    }
                    None => break, // UI dropped the sender (intentional disconnect)
                },
                incoming = read.next() => match incoming {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(sm) = serde_json::from_str::<ServerMsg>(&text) {
                            if in_tx.send(Incoming::Server(sm)).is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        let _ = in_tx.send(Incoming::Disconnected("connection closed".into()));
                        break;
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => {
                        let _ = in_tx.send(Incoming::Disconnected(format!("{e}")));
                        break;
                    }
                },
            }
        }
    });

    out_tx
}

fn http_to_ws(server: &str) -> String {
    let s = server.trim_end_matches('/');
    let base = if let Some(rest) = s.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = s.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        format!("ws://{s}")
    };
    format!("{base}/ws")
}
