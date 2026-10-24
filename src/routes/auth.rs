use axum::{
    extract::State,
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde_json::json;
use uuid::Uuid;

use crate::{
    appstate::AppState,
    db,
    models::user::{AuthResponse, LoginRequest, RegisterRequest, User, UserWithPassword},
    services::auth::{create_jwt, hash_password, verify_jwt, verify_password},
};

pub fn auth_routes() -> Router<AppState> {
    Router::new()
        .route("/register", post(register_handler))
        .route("/login", post(login_handler))
        .route("/me", get(me_handler))
}

pub async fn register_handler(
    State(state): State<AppState>,
    Json(payload): Json<RegisterRequest>,
) -> impl IntoResponse {
    let username = payload.username.trim().to_lowercase();
    let email = payload.email.trim().to_lowercase();
    let display_name = payload
        .display_name
        .or(payload.name)
        .unwrap_or_else(|| username.clone());
    let role = payload.role.unwrap_or_else(|| "user".to_string());
    let company = payload.company.map(|c| c.trim().to_string()).filter(|c| !c.is_empty());

    if username.is_empty() || email.is_empty() || payload.password.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "message": "Username, email, dan kata sandi wajib diisi",
                "code": "MISSING_REQUIRED_FIELDS"
            })),
        );
    }

    if payload.password.len() < 6 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "message": "Kata sandi minimal 6 karakter",
                "code": "PASSWORD_TOO_SHORT"
            })),
        );
    }

    // Check if username or email already exists
    let existing = db::query_as::<sqlx::Postgres, UserWithPassword>(
        "SELECT id, display_name, username, email, password_hash, role, company, created_at, updated_at 
         FROM convena.users WHERE username = $1 OR email = $2",
    )
    .bind(&username)
    .bind(&email)
    .fetch_optional(&state.pool)
    .await;

    match existing {
        Ok(Some(user)) => {
            let message = if user.username == username {
                "Username sudah digunakan oleh akun lain"
            } else {
                "Email sudah terdaftar. Silakan login"
            };
            return (
                StatusCode::CONFLICT,
                Json(json!({ "message": message, "code": "USER_ALREADY_EXISTS" })),
            );
        }
        Err(e) => {
            tracing::error!("Database query error on register: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Terjadi kesalahan sistem saat memeriksa data", "code": "DB_ERROR" })),
            );
        }
        Ok(None) => {}
    }

    let password_hash = match hash_password(&payload.password) {
        Ok(hash) => hash,
        Err(e) => {
            tracing::error!("Argon2 hash error: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal mengenkripsi kata sandi", "code": "HASH_ERROR" })),
            );
        }
    };

    let user_id = Uuid::new_v4();
    let now = chrono::Utc::now();

    let insert_result = db::query(
        "INSERT INTO convena.users (id, display_name, username, email, password_hash, role, company, created_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
    )
    .bind(user_id)
    .bind(&display_name)
    .bind(&username)
    .bind(&email)
    .bind(&password_hash)
    .bind(&role)
    .bind(&company)
    .bind(now)
    .bind(now)
    .execute(&state.pool)
    .await;

    if let Err(e) = insert_result {
        tracing::error!("Database insert error: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "message": "Gagal menyimpan akun baru", "code": "DB_INSERT_ERROR" })),
        );
    }

    let token = match create_jwt(&user_id.to_string(), &role, &state.config.jwt_secret) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("JWT creation error: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal menghasilkan token autentikasi", "code": "TOKEN_ERROR" })),
            );
        }
    };

    let user = User::from_db(
        user_id,
        display_name,
        username,
        email,
        role,
        company,
        now,
        now,
    );

    (
        StatusCode::CREATED,
        Json(json!(AuthResponse { token, user })),
    )
}

pub async fn login_handler(
    State(state): State<AppState>,
    Json(payload): Json<LoginRequest>,
) -> impl IntoResponse {
    let identifier = payload.identifier.trim().to_lowercase();
    let password = payload.password;

    if identifier.is_empty() || password.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "message": "Username/email dan kata sandi wajib diisi",
                "code": "MISSING_REQUIRED_FIELDS"
            })),
        );
    }

    let user_record = db::query_as::<sqlx::Postgres, UserWithPassword>(
        "SELECT id, display_name, username, email, password_hash, role, company, created_at, updated_at 
         FROM convena.users WHERE username = $1 OR email = $1",
    )
    .bind(&identifier)
    .fetch_optional(&state.pool)
    .await;

    let user = match user_record {
        Ok(Some(u)) => u,
        Ok(None) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "message": "Username/email atau kata sandi tidak cocok",
                    "code": "INVALID_CREDENTIALS"
                })),
            );
        }
        Err(e) => {
            tracing::error!("Database query error on login: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Terjadi kesalahan basis data", "code": "DB_ERROR" })),
            );
        }
    };

    if !verify_password(&password, &user.password_hash) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({
                "message": "Username/email atau kata sandi tidak cocok",
                "code": "INVALID_CREDENTIALS"
            })),
        );
    }

    let token = match create_jwt(&user.id.to_string(), &user.role, &state.config.jwt_secret) {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("JWT creation error on login: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal menghasilkan token autentikasi", "code": "TOKEN_ERROR" })),
            );
        }
    };

    let user_dto = User::from_db(
        user.id,
        user.display_name,
        user.username,
        user.email,
        user.role,
        user.company,
        user.created_at,
        user.updated_at,
    );

    (
        StatusCode::OK,
        Json(json!(AuthResponse {
            token,
            user: user_dto
        })),
    )
}

pub async fn me_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let auth_header = match headers.get(AUTHORIZATION).and_then(|v| v.to_str().ok()) {
        Some(h) => h,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Header otorisasi tidak ditemukan", "code": "UNAUTHORIZED" })),
            );
        }
    };

    let token = match auth_header.strip_prefix("Bearer ") {
        Some(t) => t.trim(),
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Format token Bearer tidak valid", "code": "INVALID_TOKEN_FORMAT" })),
            );
        }
    };

    let claims = match verify_jwt(token, &state.config.jwt_secret) {
        Ok(c) => c,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Token tidak valid atau telah kedaluwarsa", "code": "TOKEN_EXPIRED" })),
            );
        }
    };

    let user_id = match Uuid::parse_str(&claims.sub) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Identitas pengguna dalam token tidak valid", "code": "INVALID_TOKEN_SUBJECT" })),
            );
        }
    };

    let user_record = db::query_as::<sqlx::Postgres, UserWithPassword>(
        "SELECT id, display_name, username, email, password_hash, role, company, created_at, updated_at 
         FROM convena.users WHERE id = $1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await;

    match user_record {
        Ok(Some(u)) => {
            let user_dto = User::from_db(
                u.id,
                u.display_name,
                u.username,
                u.email,
                u.role,
                u.company,
                u.created_at,
                u.updated_at,
            );
            (StatusCode::OK, Json(json!(user_dto)))
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": "Pengguna tidak ditemukan", "code": "USER_NOT_FOUND" })),
        ),
        Err(e) => {
            tracing::error!("Database query error on me: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan basis data", "code": "DB_ERROR" })),
            )
        }
    }
}
