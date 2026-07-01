//! SQLite persistence layer. Uses the runtime query API (not the compile-time
//! `query!` macros) so the project builds without a live database at compile time.

use anyhow::Result;
use protocol::{Attachment, DirectMessage, GroupInfo, GroupMessage, UnreadCount};
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

    // File/image attachments. The bytes live here (BLOB) so a self-hosted
    // deployment keeps everything in the single SQLite file / mounted volume.
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS attachments (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            owner      TEXT NOT NULL,
            name       TEXT NOT NULL,
            mime       TEXT NOT NULL,
            size       INTEGER NOT NULL,
            data       BLOB NOT NULL,
            created_at INTEGER NOT NULL
        )"#,
    )
    .execute(pool)
    .await?;

    // Link messages to an optional attachment. Added via ALTER so existing
    // databases pick the column up without a destructive migration.
    add_column_if_missing(pool, "messages", "attachment_id", "INTEGER").await?;
    add_column_if_missing(pool, "group_messages", "attachment_id", "INTEGER").await?;

    // Per-user, per-conversation read position, so unread badges survive across
    // sessions and stay in sync across a user's devices. `conversation` is
    // `dm:<peer>` for direct chats or `grp:<id>` for groups.
    sqlx::query(
        r#"CREATE TABLE IF NOT EXISTS read_marks (
            username     TEXT NOT NULL,
            conversation TEXT NOT NULL,
            last_read_ts INTEGER NOT NULL,
            PRIMARY KEY (username, conversation)
        )"#,
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Add `column` to `table` if it isn't there yet. SQLite has no
/// `ADD COLUMN IF NOT EXISTS`, so we tolerate the "duplicate column" error.
async fn add_column_if_missing(pool: &SqlitePool, table: &str, column: &str, ty: &str) -> Result<()> {
    let sql = format!("ALTER TABLE {table} ADD COLUMN {column} {ty}");
    match sqlx::query(&sql).execute(pool).await {
        Ok(_) => Ok(()),
        // Already present — nothing to do.
        Err(sqlx::Error::Database(e)) if e.message().contains("duplicate column name") => Ok(()),
        Err(e) => Err(e.into()),
    }
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
    sqlx::query("INSERT INTO messages (sender, recipient, body, ts, attachment_id) VALUES (?, ?, ?, ?, ?)")
        .bind(&msg.from)
        .bind(&msg.to)
        .bind(&msg.body)
        .bind(msg.ts)
        .bind(msg.attachment.as_ref().map(|a| a.id))
        .execute(pool)
        .await?;
    Ok(())
}

/// A direct-message row joined with its optional attachment metadata.
type DmRow = (String, String, String, i64, Option<i64>, Option<String>, Option<String>, Option<i64>);
/// A group-message row joined with its optional attachment metadata.
type GroupRow = (String, String, i64, Option<i64>, Option<String>, Option<String>, Option<i64>);

fn attachment_from_cols(id: Option<i64>, name: Option<String>, mime: Option<String>, size: Option<i64>) -> Option<Attachment> {
    match (id, name, mime, size) {
        (Some(id), Some(name), Some(mime), Some(size)) => Some(Attachment { id, name, mime, size }),
        _ => None,
    }
}

/// Most recent messages this user is party to (sent or received), oldest first.
pub async fn recent_for_user(pool: &SqlitePool, username: &str, limit: i64) -> Result<Vec<DirectMessage>> {
    let rows: Vec<DmRow> = sqlx::query_as(
        r#"SELECT m.sender, m.recipient, m.body, m.ts, a.id, a.name, a.mime, a.size
           FROM messages m LEFT JOIN attachments a ON a.id = m.attachment_id
           WHERE m.sender = ?1 OR m.recipient = ?1
           ORDER BY m.ts DESC LIMIT ?2"#,
    )
    .bind(username)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut out: Vec<DirectMessage> = rows
        .into_iter()
        .map(|(from, to, body, ts, aid, aname, amime, asize)| DirectMessage {
            from,
            to,
            body,
            ts,
            attachment: attachment_from_cols(aid, aname, amime, asize),
        })
        .collect();
    out.reverse(); // oldest first for replay
    Ok(out)
}

// === Attachments ============================================================

/// Store an uploaded file's bytes and return its metadata (with the new id).
pub async fn store_attachment(
    pool: &SqlitePool,
    owner: &str,
    name: &str,
    mime: &str,
    data: &[u8],
) -> Result<Attachment> {
    let size = data.len() as i64;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO attachments (owner, name, mime, size, data, created_at) VALUES (?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(mime)
    .bind(size)
    .bind(data)
    .bind(chrono::Utc::now().timestamp())
    .fetch_one(pool)
    .await?;
    Ok(Attachment { id, name: name.to_string(), mime: mime.to_string(), size })
}

/// Metadata for an attachment (no bytes), used to embed it in a message.
pub async fn attachment_meta(pool: &SqlitePool, id: i64) -> Result<Option<Attachment>> {
    let row: Option<(i64, String, String, i64)> =
        sqlx::query_as("SELECT id, name, mime, size FROM attachments WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(id, name, mime, size)| Attachment { id, name, mime, size }))
}

/// Fetch an attachment's bytes and (name, mime) for download.
pub async fn attachment_bytes(pool: &SqlitePool, id: i64) -> Result<Option<(String, String, Vec<u8>)>> {
    let row: Option<(String, String, Vec<u8>)> =
        sqlx::query_as("SELECT name, mime, data FROM attachments WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    Ok(row)
}

// === Read marks / unread ====================================================

/// Advance a user's read position for a conversation (never moves it backwards).
async fn mark_read(pool: &SqlitePool, username: &str, conversation: &str, ts: i64) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO read_marks (username, conversation, last_read_ts) VALUES (?1, ?2, ?3)
           ON CONFLICT(username, conversation)
           DO UPDATE SET last_read_ts = MAX(last_read_ts, excluded.last_read_ts)"#,
    )
    .bind(username)
    .bind(conversation)
    .bind(ts)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn mark_read_dm(pool: &SqlitePool, username: &str, peer: &str, ts: i64) -> Result<()> {
    mark_read(pool, username, &format!("dm:{peer}"), ts).await
}

pub async fn mark_read_group(pool: &SqlitePool, username: &str, group_id: i64, ts: i64) -> Result<()> {
    mark_read(pool, username, &format!("grp:{group_id}"), ts).await
}

/// Count unread messages per conversation for `username`: incoming DMs and
/// group messages (from others) newer than the stored read position.
pub async fn unread_counts(pool: &SqlitePool, username: &str) -> Result<Vec<UnreadCount>> {
    let mut out = Vec::new();

    let dm_rows: Vec<(String, i64)> = sqlx::query_as(
        r#"SELECT m.sender, COUNT(*)
           FROM messages m
           LEFT JOIN read_marks r
             ON r.username = ?1 AND r.conversation = 'dm:' || m.sender
           WHERE m.recipient = ?1 AND m.sender <> ?1
             AND m.ts > COALESCE(r.last_read_ts, 0)
           GROUP BY m.sender"#,
    )
    .bind(username)
    .fetch_all(pool)
    .await?;
    for (peer, count) in dm_rows {
        out.push(UnreadCount { peer: Some(peer), group_id: None, count: count as u32 });
    }

    let grp_rows: Vec<(i64, i64)> = sqlx::query_as(
        r#"SELECT gm.group_id, COUNT(*)
           FROM group_messages gm
           JOIN group_members mem
             ON mem.group_id = gm.group_id AND mem.username = ?1
           LEFT JOIN read_marks r
             ON r.username = ?1 AND r.conversation = 'grp:' || gm.group_id
           WHERE gm.sender <> ?1
             AND gm.ts > COALESCE(r.last_read_ts, 0)
           GROUP BY gm.group_id"#,
    )
    .bind(username)
    .fetch_all(pool)
    .await?;
    for (group_id, count) in grp_rows {
        out.push(UnreadCount { peer: None, group_id: Some(group_id), count: count as u32 });
    }

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
    sqlx::query("INSERT INTO group_messages (group_id, sender, body, ts, attachment_id) VALUES (?, ?, ?, ?, ?)")
        .bind(msg.group_id)
        .bind(&msg.from)
        .bind(&msg.body)
        .bind(msg.ts)
        .bind(msg.attachment.as_ref().map(|a| a.id))
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn recent_group_messages(pool: &SqlitePool, group_id: i64, limit: i64) -> Result<Vec<GroupMessage>> {
    let rows: Vec<GroupRow> = sqlx::query_as(
        r#"SELECT gm.sender, gm.body, gm.ts, a.id, a.name, a.mime, a.size
           FROM group_messages gm LEFT JOIN attachments a ON a.id = gm.attachment_id
           WHERE gm.group_id = ?1 ORDER BY gm.ts DESC LIMIT ?2"#,
    )
    .bind(group_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let mut out: Vec<GroupMessage> = rows
        .into_iter()
        .map(|(from, body, ts, aid, aname, amime, asize)| GroupMessage {
            group_id,
            from,
            body,
            ts,
            attachment: attachment_from_cols(aid, aname, amime, asize),
        })
        .collect();
    out.reverse();
    Ok(out)
}
