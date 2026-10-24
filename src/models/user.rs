use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: Uuid,
    pub display_name: String,
    #[serde(default)]
    pub name: String,
    pub username: String,
    pub email: String,
    pub role: String,
    pub company: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl User {
    pub fn from_db(
        id: Uuid,
        display_name: String,
        username: String,
        email: String,
        role: String,
        company: Option<String>,
        created_at: DateTime<Utc>,
        updated_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            name: display_name.clone(),
            display_name,
            username,
            email,
            role,
            company,
            created_at,
            updated_at,
        }
    }
}

#[derive(Debug, Clone, FromRow)]
pub struct UserWithPassword {
    pub id: Uuid,
    pub display_name: String,
    pub username: String,
    pub email: String,
    pub password_hash: String,
    pub role: String,
    pub company: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct RegisterRequest {
    pub name: Option<String>,
    pub display_name: Option<String>,
    pub username: String,
    pub email: String,
    pub password: String,
    pub role: Option<String>,
    pub company: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub identifier: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthResponse {
    pub token: String,
    pub user: User,
}
