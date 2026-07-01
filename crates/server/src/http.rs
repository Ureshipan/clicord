//! HTTP handlers for registration, login and attachment upload/download.

use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use protocol::{Attachment, AuthRequest, AuthResponse};
use serde::Deserialize;

use crate::{auth, db, AppState};

type ApiError = (StatusCode, String);

fn internal<E: std::fmt::Display>(e: E) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

pub async fn register(
    State(st): State<AppState>,
    Json(req): Json<AuthRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    validate_credentials(&req)?;

    if db::user_exists(&st.db, &req.username).await.map_err(internal)? {
        return Err((StatusCode::CONFLICT, "username already taken".into()));
    }

    let hash = auth::hash_password(&req.password).map_err(internal)?;
    db::create_user(&st.db, &req.username, &hash).await.map_err(internal)?;

    let token = auth::make_token(&st.jwt_secret, &req.username).map_err(internal)?;
    Ok(Json(AuthResponse { token, username: req.username }))
}

pub async fn login(
    State(st): State<AppState>,
    Json(req): Json<AuthRequest>,
) -> Result<Json<AuthResponse>, ApiError> {
    validate_credentials(&req)?;

    let stored = db::password_hash(&st.db, &req.username)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::UNAUTHORIZED, "invalid credentials".to_string()))?;

    if !auth::verify_password(&req.password, &stored) {
        return Err((StatusCode::UNAUTHORIZED, "invalid credentials".into()));
    }

    let token = auth::make_token(&st.jwt_secret, &req.username).map_err(internal)?;
    Ok(Json(AuthResponse { token, username: req.username }))
}

// === Attachments ============================================================

/// Extract and verify the bearer token from the `Authorization` header,
/// returning the authenticated username.
fn authed_user(st: &AppState, headers: &HeaderMap) -> Result<String, ApiError> {
    let token = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or((StatusCode::UNAUTHORIZED, "missing bearer token".to_string()))?;
    auth::verify_token(&st.jwt_secret, token)
        .map_err(|_| (StatusCode::UNAUTHORIZED, "invalid token".to_string()))
}

#[derive(Deserialize)]
pub struct UploadParams {
    /// Original file name (basename); sanitized server-side.
    name: String,
    /// MIME type; defaults to a generic binary type.
    #[serde(default)]
    mime: Option<String>,
}

/// `POST /api/upload?name=<file>&mime=<type>` with the raw bytes as the body.
/// Returns the stored attachment's metadata. Body size is capped by the route's
/// `DefaultBodyLimit`.
pub async fn upload(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<UploadParams>,
    body: Bytes,
) -> Result<Json<Attachment>, ApiError> {
    let user = authed_user(&st, &headers)?;
    if body.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "empty file".into()));
    }
    // Keep only the basename so a path can't smuggle directories into the name.
    let name = sanitize_name(&params.name);
    let mime = params
        .mime
        .filter(|m| !m.is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string());

    let att = db::store_attachment(&st.db, &user, &name, &mime, &body)
        .await
        .map_err(internal)?;
    Ok(Json(att))
}

/// `GET /api/attachment/:id` — stream an attachment's bytes back to a client.
pub async fn download(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<i64>,
) -> Result<Response, ApiError> {
    authed_user(&st, &headers)?;
    let (name, mime, data) = db::attachment_bytes(&st.db, id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "no such attachment".to_string()))?;

    let disposition = format!("attachment; filename=\"{}\"", sanitize_name(&name));
    let resp = (
        [
            (header::CONTENT_TYPE, mime),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        data,
    )
        .into_response();
    Ok(resp)
}

/// Reduce a possibly path-like name to a safe basename.
fn sanitize_name(name: &str) -> String {
    let base = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(name)
        .trim()
        .trim_matches('.');
    if base.is_empty() {
        "file".to_string()
    } else {
        base.chars().take(255).collect()
    }
}

fn validate_credentials(req: &AuthRequest) -> Result<(), ApiError> {
    let name = req.username.trim();
    if name.is_empty() || name.len() > 32 {
        return Err((StatusCode::BAD_REQUEST, "username must be 1-32 chars".into()));
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err((
            StatusCode::BAD_REQUEST,
            "username may only contain a-z, 0-9, _ and -".into(),
        ));
    }
    if req.password.len() < 6 {
        return Err((StatusCode::BAD_REQUEST, "password must be at least 6 chars".into()));
    }
    Ok(())
}
