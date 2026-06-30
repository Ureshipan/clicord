//! SQLite persistence layer. Uses the runtime query API (not the compile-time
//! `query!` macros) so the project builds without a live database at compile time.

use anyhow::Result;
use protocol::{DirectMessage, GroupInfo, GroupMessage};
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

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS groups (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            name       TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )"#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS group_members (
            group_id INTEGER NOT NULL,
            username TEXT NOT NULL,
            PRIMARY KEY (group_id, username)
        )"#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS group_messages (
            id       INTEGER PRIMARY KEY AUTOINCREMENT,
            group_id INTEGER NOT NULL,
            sender   TEXT NOT NULL,
            body     TEXT NOT NULL,
            ts       INTEGER NOT NULL
        )"#,
    )
    .execute(pool)
    .await?;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_group_messages ON group_messages (group_id)")
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

// === Users / search =========================================================

/// Usernames starting with `query` (case-insensitive), capped at `limit`.
pub async fn search_users(pool: &SqlitePool, query: &str, limit: i64) -> Result<Vec<String>> {
    // Escape LIKE wildcards in the user-supplied prefix.
    let escaped = query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    let pattern = format!("{escaped}%");
    let rows: Vec<(String,)> = sqlx::query_as(
        r#"SELECT username FROM users
           WHERE username LIKE ?1 ESCAPE '\'
           ORDER BY username LIMIT ?2"#,
    )
    .bind(pattern)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

// === Groups =================================================================

pub async fn create_group(pool: &SqlitePool, name: &str, members: &[String]) -> Result<GroupInfo> {
    let now = chrono::Utc::now().timestamp();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO groups (name, created_at) VALUES (?, ?) RETURNING id",
    )
    .bind(name)
    .bind(now)
    .fetch_one(pool)
    .await?;

    for m in members {
        sqlx::query("INSERT OR IGNORE INTO group_members (group_id, username) VALUES (?, ?)")
            .bind(id)
            .bind(m)
            .execute(pool)
            .await?;
    }

    Ok(GroupInfo {
        id,
        name: name.to_string(),
        members: group_members(pool, id).await?,
    })
}

pub async fn group_members(pool: &SqlitePool, group_id: i64) -> Result<Vec<String>> {
    let rows: Vec<(String,)> =
        sqlx::query_as("SELECT username FROM group_members WHERE group_id = ? ORDER BY username")
            .bind(group_id)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

pub async fn is_member(pool: &SqlitePool, group_id: i64, username: &str) -> Result<bool> {
    let row: Option<(i64,)> =
        sqlx::query_as("SELECT 1 FROM group_members WHERE group_id = ? AND username = ?")
            .bind(group_id)
            .bind(username)
            .fetch_optional(pool)
            .await?;
    Ok(row.is_some())
}

/// All groups a user belongs to, each with its member list.
pub async fn groups_for_user(pool: &SqlitePool, username: &str) -> Result<Vec<GroupInfo>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        r#"SELECT g.id, g.name FROM groups g
           JOIN group_members m ON m.group_id = g.id
           WHERE m.username = ? ORDER BY g.id"#,
    )
    .bind(username)
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for (id, name) in rows {
        out.push(GroupInfo {
            id,
            name,
            members: group_members(pool, id).await?,
        });
    }
    Ok(out)
}

pub async fn store_group_message(pool: &SqlitePool, msg: &GroupMessage) -> Result<()> {
    sqlx::query("INSERT INTO group_messages (group_id, sender, body, ts) VALUES (?, ?, ?, ?)")
        .bind(msg.group_id)
        .bind(&msg.from)
        .bind(&msg.body)
        .bind(msg.ts)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn recent_group_messages(pool: &SqlitePool, group_id: i64, limit: i64) -> Result<Vec<GroupMessage>> {
    let rows: Vec<(String, String, i64)> = sqlx::query_as(
        r#"SELECT sender, body, ts FROM group_messages
           WHERE group_id = ?1 ORDER BY ts DESC LIMIT ?2"#,
    )
    .bind(group_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut out: Vec<GroupMessage> = rows
        .into_iter()
        .map(|(from, body, ts)| GroupMessage { group_id, from, body, ts })
        .collect();
    out.reverse();
    Ok(out)
}
