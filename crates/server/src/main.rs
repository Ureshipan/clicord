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

use axum::extract::DefaultBodyLimit;
use axum::routing::{get, post};
use axum::Router;
use std::sync::Arc;
use tracing_subscriber::EnvFilter;

/// Maximum size of a single uploaded attachment.
const MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;

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

    let db = db::connect(&db_url).await?;

    // The JWT secret never lives in the repo or config. An explicit
    // CLICORD_JWT_SECRET (e.g. a samoswallow encrypted Secret) wins; otherwise
    // we use a random secret persisted in the database under /data.
    let jwt_secret = match std::env::var("CLICORD_JWT_SECRET") {
        Ok(s) if !s.is_empty() => {
            tracing::info!("using JWT secret from CLICORD_JWT_SECRET");
            s
        }
        _ => {
            tracing::info!("CLICORD_JWT_SECRET unset — using persisted secret from the database");
            db::get_or_create_jwt_secret(&db).await?
        }
    };

    let state = AppState {
        db,
        jwt_secret: Arc::new(jwt_secret),
        hub: hub::Hub::default(),
    };

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/register", post(http::register))
        .route("/api/login", post(http::login))
        .route(
            "/api/upload",
            post(http::upload).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route("/api/attachment/:id", get(http::download))
        .route("/ws", get(ws::ws_handler))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&listen).await?;
    tracing::info!("clicord-server listening on http://{listen}");
    axum::serve(listener, app).await?;
    Ok(())
}
