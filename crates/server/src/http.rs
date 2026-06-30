//! HTTP handlers for registration and login.

use axum::{extract::State, http::StatusCode, Json};
use protocol::{AuthRequest, AuthResponse};

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
