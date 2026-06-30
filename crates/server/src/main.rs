//! clicord-server: auth + message routing for the clicord messenger.
//!
//! Public surface is a single HTTP port (works behind the samoswallow/Caddy
//! reverse proxy): `/health`, `/api/register`, `/api/login`, and the `/ws`
//! websocket used for realtime messaging.

mod auth;
mod db;
mod hub;
mod http;
mod ws;

use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
pub struct AppState {
    pub db: sqlx::SqlitePool,
    pub jwt_secret: Arc<String>,
    pub hub: hub::Hub,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    // Configuration via env vars — these map cleanly onto samoswallow's `env:` block.
    let listen = env_or("CLICORD_LISTEN", "0.0.0.0:8080");
    let db_url = env_or("CLICORD_DB", "sqlite://clicord.db");
    let jwt_secret = env_or("CLICORD_JWT_SECRET", "dev-insecure-secret-change-me");
    if jwt_secret == "dev-insecure-secret-change-me" {
        tracing::warn!("CLICORD_JWT_SECRET is unset — using an insecure development secret");
    }

    let db = db::connect(&db_url).await?;
    let state = AppState {
        db,
        jwt_secret: Arc::new(jwt_secret),
        hub: hub::Hub::default(),
    };

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/register", post(http::register))
        .route("/api/login", post(http::login))
        .route("/ws", get(ws::ws_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&listen).await?;
    tracing::info!("clicord-server listening on http://{listen}");
    axum::serve(listener, app).await?;
    Ok(())
}
