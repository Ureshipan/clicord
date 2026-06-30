//! SQLite persistence layer. Uses the runtime query API (not the compile-time
//! `query!` macros) so the project builds without a live database at compile time.

use anyhow::Result;
use protocol::DirectMessage;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;

/// Open (creating if needed) the SQLite database and run migrations.
pub async fn connect(url: &str) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(url)?.create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;
    migrate(&pool).await?;
    Ok(pool)
}

async fn migrate(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS users (
            username      TEXT PRIMARY KEY,
            password_hash TEXT NOT NULL,
            created_at    INTEGER NOT NULL
        )"#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS messages (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            sender    TEXT NOT NULL,
            recipient TEXT NOT NULL,
            body      TEXT NOT NULL,
            ts        INTEGER NOT NULL
        )"#,
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_messages_pair ON messages (sender, recipient)")
        .execute(pool)
        .await?;

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS meta (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )"#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Return the persisted JWT signing secret, generating and storing a random
/// one on first run. Persisted in the database (i.e. under /data in
/// production), so it survives restarts but never lives in the repo or config.
pub async fn get_or_create_jwt_secret(pool: &SqlitePool) -> Result<String> {
    let existing: Option<(String,)> = sqlx::query_as("SELECT value FROM meta WHERE key = 'jwt_secret'")
        .fetch_optional(pool)
        .await?;
    if let Some((secret,)) = existing {
        return Ok(secret);
    }

    let secret = random_hex_32();
    // INSERT OR IGNORE guards against a race between two concurrent boots.
    sqlx::query("INSERT OR IGNORE INTO meta (key, value) VALUES ('jwt_secret', ?)")
        .bind(&secret)
        .execute(pool)
        .await?;

    // Re-read so every instance ends up with the same persisted value.
    let (secret,): (String,) = sqlx::query_as("SELECT value FROM meta WHERE key = 'jwt_secret'")
        .fetch_one(pool)
        .await?;
    Ok(secret)
}

fn random_hex_32() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub async fn user_exists(pool: &SqlitePool, username: &str) -> Result<bool> {
    let row: Option<(i64,)> = sqlx::query_as("SELECT 1 FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

pub async fn create_user(pool: &SqlitePool, username: &str, password_hash: &str) -> Result<()> {
    sqlx::query("INSERT INTO users (username, password_hash, created_at) VALUES (?, ?, ?)")
        .bind(username)
        .bind(password_hash)
        .bind(chrono::Utc::now().timestamp())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn password_hash(pool: &SqlitePool, username: &str) -> Result<Option<String>> {
    let row: Option<(String,)> = sqlx::query_as("SELECT password_hash FROM users WHERE username = ?")
        .bind(username)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| r.0))
}

pub async fn store_message(pool: &SqlitePool, msg: &DirectMessage) -> Result<()> {
    sqlx::query("INSERT INTO messages (sender, recipient, body, ts) VALUES (?, ?, ?, ?)")
        .bind(&msg.from)
        .bind(&msg.to)
        .bind(&msg.body)
        .bind(msg.ts)
        .execute(pool)
        .await?;
    Ok(())
}

/// Most recent messages this user is party to (sent or received), oldest first.
pub async fn recent_for_user(pool: &SqlitePool, username: &str, limit: i64) -> Result<Vec<DirectMessage>> {
    let rows: Vec<(String, String, String, i64)> = sqlx::query_as(
        r#"SELECT sender, recipient, body, ts FROM messages
           WHERE sender = ?1 OR recipient = ?1
           ORDER BY ts DESC LIMIT ?2"#,
    )
    .bind(username)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut out: Vec<DirectMessage> = rows
        .into_iter()
        .map(|(from, to, body, ts)| DirectMessage { from, to, body, ts })
        .collect();
    out.reverse(); // oldest first for replay
    Ok(out)
}
