use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, patch, post},
    Json, Router,
};
use serde_json::json;
use uuid::Uuid;

use crate::{
    appstate::AppState,
    db,
    models::{
        event::{CreateEventPayload, Event, EventWithRole},
        membership::{
            EventMemberDetail, EventPermission, EventRole, InviteMemberPayload,
            UpdateMemberRolePayload,
        },
    },
    services::{
        auth::extract_user_id_from_headers,
        rbac::{check_event_permission, get_user_event_role},
    },
};

pub fn event_routes() -> Router<AppState> {
    Router::new()
        .route("/", post(create_event_handler).get(list_events_handler))
        .route("/my-events", get(list_my_events_handler))
        .route("/{id}", get(get_event_handler))
        .route("/{id}/members", get(list_members_handler).post(invite_member_handler))
        .route(
            "/{id}/members/{user_id}",
            patch(update_member_handler).delete(remove_member_handler),
        )
}

fn generate_slug(title: &str) -> String {
    let clean: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let trimmed = clean.trim_matches('-');
    let suffix = &Uuid::new_v4().to_string()[..6];
    format!("{trimmed}-{suffix}")
}

/// Create a new event and automatically make the creator the 'owner'.
pub async fn create_event_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CreateEventPayload>,
) -> impl IntoResponse {
    let current_user_id = match extract_user_id_from_headers(&headers, &state.config.jwt_secret) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Otorisasi diperlukan untuk membuat event", "code": "UNAUTHORIZED" })),
            );
        }
    };

    let title = payload.title.trim().to_string();
    if title.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Judul event wajib diisi", "code": "MISSING_TITLE" })),
        );
    }

    let slug = payload
        .slug
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| generate_slug(&title));

    let event_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    let location_type = payload.location_type.unwrap_or_else(|| "offline".to_string());

    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!("Failed to start transaction: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan basis data", "code": "DB_ERROR" })),
            );
        }
    };

    // 1. Insert Event
    let insert_event_sql = r#"
        INSERT INTO convena.events (
            id, created_by, title, slug, description, short_description, banner_url,
            category, status, visibility, start_date, end_date, location_type,
            venue_name, address, city, online_url, organization_name, university,
            created_at, updated_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, 'draft', 'public', $9, $10, $11,
            $12, $13, $14, $15, $16, $17, $18, $18
        ) RETURNING id, created_by, title, slug, description, short_description, banner_url,
                    category, status, visibility, start_date, end_date, location_type,
                    venue_name, address, city, online_url, organization_name, university,
                    created_at, updated_at
    "#;

    let event = match db::query_as::<sqlx::Postgres, Event>(insert_event_sql)
        .bind(event_id)
        .bind(current_user_id)
        .bind(&title)
        .bind(&slug)
        .bind(payload.description)
        .bind(payload.short_description)
        .bind(payload.banner_url)
        .bind(payload.category)
        .bind(payload.start_date)
        .bind(payload.end_date)
        .bind(location_type)
        .bind(payload.venue_name)
        .bind(payload.address)
        .bind(payload.city)
        .bind(payload.online_url)
        .bind(payload.organization_name)
        .bind(payload.university)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
    {
        Ok(ev) => ev,
        Err(e) => {
            tracing::error!("Failed to insert event: {e}");
            let _ = tx.rollback().await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal menyimpan event ke basis data", "code": "DB_ERROR" })),
            );
        }
    };

    // 2. Automatically assign creator as 'owner' in convena.event_members
    let member_id = Uuid::new_v4();
    let insert_member_sql = r#"
        INSERT INTO convena.event_members (
            id, event_id, user_id, role, custom_title, status, joined_at, updated_at
        ) VALUES (
            $1, $2, $3, 'owner', 'Ketua / Creator', 'active', $4, $4
        )
    "#;

    if let Err(e) = db::query(insert_member_sql)
        .bind(member_id)
        .bind(event_id)
        .bind(current_user_id)
        .bind(now)
        .execute(&mut *tx)
        .await
    {
        tracing::error!("Failed to insert event owner: {e}");
        let _ = tx.rollback().await;
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "message": "Gagal mengalokasikan role pemilik event", "code": "DB_ERROR" })),
        );
    }

    if let Err(e) = tx.commit().await {
        tracing::error!("Failed to commit transaction: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "message": "Gagal menyimpan transaksi event", "code": "DB_ERROR" })),
        );
    }

    (
        StatusCode::CREATED,
        Json(json!({
            "event": event,
            "role": "owner"
        })),
    )
}

/// List all events for the current user along with their respective role in each event.
pub async fn list_my_events_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let current_user_id = match extract_user_id_from_headers(&headers, &state.config.jwt_secret) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Otorisasi diperlukan", "code": "UNAUTHORIZED" })),
            );
        }
    };

    let sql = r#"
        SELECT 
            e.id, e.created_by, e.title, e.slug, e.description, e.short_description, e.banner_url,
            e.category, e.status, e.visibility, e.start_date, e.end_date, e.location_type,
            e.venue_name, e.organization_name, e.university, e.created_at,
            m.role as my_role, m.custom_title as my_custom_title
        FROM convena.events e
        INNER JOIN convena.event_members m ON e.id = m.event_id
        WHERE m.user_id = $1 AND m.status = 'active'
        ORDER BY e.created_at DESC
    "#;

    match db::query_as::<sqlx::Postgres, EventWithRole>(sql)
        .bind(current_user_id)
        .fetch_all(&state.pool)
        .await
    {
        Ok(events) => (StatusCode::OK, Json(json!(events))),
        Err(e) => {
            tracing::error!("Failed to list my events: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal memuat daftar event", "code": "DB_ERROR" })),
            )
        }
    }
}

/// Public list of events.
pub async fn list_events_handler(State(state): State<AppState>) -> impl IntoResponse {
    let sql = r#"
        SELECT 
            id, created_by, title, slug, description, short_description, banner_url,
            category, status, visibility, start_date, end_date, location_type,
            venue_name, address, city, online_url, organization_name, university,
            created_at, updated_at
        FROM convena.events
        WHERE visibility = 'public'
        ORDER BY start_date ASC
        LIMIT 50
    "#;

    match db::query_as::<sqlx::Postgres, Event>(sql)
        .fetch_all(&state.pool)
        .await
    {
        Ok(events) => (StatusCode::OK, Json(json!(events))),
        Err(e) => {
            tracing::error!("Failed to list public events: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal memuat event", "code": "DB_ERROR" })),
            )
        }
    }
}

/// Get single event detail.
pub async fn get_event_handler(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> impl IntoResponse {
    let sql = r#"
        SELECT 
            id, created_by, title, slug, description, short_description, banner_url,
            category, status, visibility, start_date, end_date, location_type,
            venue_name, address, city, online_url, organization_name, university,
            created_at, updated_at
        FROM convena.events
        WHERE id = $1
    "#;

    match db::query_as::<sqlx::Postgres, Event>(sql)
        .bind(id)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(Some(event)) => (StatusCode::OK, Json(json!(event))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": "Event tidak ditemukan", "code": "EVENT_NOT_FOUND" })),
        ),
        Err(e) => {
            tracing::error!("Failed to get event: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan basis data", "code": "DB_ERROR" })),
            )
        }
    }
}

/// List all members for an event. Requires TeamViewMembers permission.
pub async fn list_members_handler(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let current_user_id = match extract_user_id_from_headers(&headers, &state.config.jwt_secret) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Otorisasi diperlukan", "code": "UNAUTHORIZED" })),
            );
        }
    };

    let can_view = match check_event_permission(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::TeamViewMembers,
    )
    .await
    {
        Ok(allowed) => allowed,
        Err(e) => {
            tracing::error!("Failed to check permissions: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan otorisasi", "code": "AUTH_ERROR" })),
            );
        }
    };

    if !can_view {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "message": "Anda tidak memiliki izin melihat susunan panitia event ini",
                "code": "FORBIDDEN"
            })),
        );
    }

    let sql = r#"
        SELECT 
            m.id, m.event_id, m.user_id, u.display_name, u.username, u.email,
            m.role, m.custom_title, m.status, m.joined_at
        FROM convena.event_members m
        INNER JOIN convena.users u ON m.user_id = u.id
        WHERE m.event_id = $1
        ORDER BY 
            CASE m.role
                WHEN 'owner' THEN 1
                WHEN 'admin' THEN 2
                WHEN 'treasurer' THEN 3
                WHEN 'committee' THEN 4
                WHEN 'sponsor' THEN 5
                ELSE 6
            END,
            m.joined_at ASC
    "#;

    match db::query_as::<sqlx::Postgres, EventMemberDetail>(sql)
        .bind(event_id)
        .fetch_all(&state.pool)
        .await
    {
        Ok(members) => (StatusCode::OK, Json(json!(members))),
        Err(e) => {
            tracing::error!("Failed to list event members: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal memuat anggota panitia", "code": "DB_ERROR" })),
            )
        }
    }
}

/// Invite or directly add a member to the event. Requires TeamInviteMember permission.
pub async fn invite_member_handler(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<InviteMemberPayload>,
) -> impl IntoResponse {
    let current_user_id = match extract_user_id_from_headers(&headers, &state.config.jwt_secret) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Otorisasi diperlukan", "code": "UNAUTHORIZED" })),
            );
        }
    };

    let can_invite = match check_event_permission(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::TeamInviteMember,
    )
    .await
    {
        Ok(allowed) => allowed,
        Err(e) => {
            tracing::error!("Failed to check permissions: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan otorisasi", "code": "AUTH_ERROR" })),
            );
        }
    };

    if !can_invite {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "message": "Hanya Owner atau Admin yang dapat mengundang anggota tim baru",
                "code": "FORBIDDEN"
            })),
        );
    }

    let role_enum = match EventRole::from_str(&payload.role) {
        Some(r) => r,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": "Role event tidak valid", "code": "INVALID_ROLE" })),
            );
        }
    };

    if role_enum == EventRole::Owner {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "message": "Tidak dapat menambahkan anggota baru sebagai Owner langsung",
                "code": "CANNOT_INVITE_OWNER"
            })),
        );
    }

    let email = payload.email.trim().to_lowercase();
    if email.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Email wajib diisi", "code": "MISSING_EMAIL" })),
        );
    }

    // Check if user exists in convena.users
    let user_lookup = db::query_as::<sqlx::Postgres, (Uuid, String, String)>(
        "SELECT id, display_name, username FROM convena.users WHERE email = $1",
    )
    .bind(&email)
    .fetch_optional(&state.pool)
    .await;

    match user_lookup {
        Ok(Some((target_user_id, target_name, target_username))) => {
            // User exists: direct add to event_members
            let member_id = Uuid::new_v4();
            let now = chrono::Utc::now();
            let insert_sql = r#"
                INSERT INTO convena.event_members (
                    id, event_id, user_id, role, custom_title, status, joined_at, updated_at
                ) VALUES (
                    $1, $2, $3, $4, $5, 'active', $6, $6
                )
                ON CONFLICT (event_id, user_id) 
                DO UPDATE SET role = EXCLUDED.role, custom_title = EXCLUDED.custom_title, updated_at = EXCLUDED.updated_at
            "#;

            if let Err(e) = db::query(insert_sql)
                .bind(member_id)
                .bind(event_id)
                .bind(target_user_id)
                .bind(role_enum.as_str())
                .bind(&payload.custom_title)
                .bind(now)
                .execute(&state.pool)
                .await
            {
                tracing::error!("Failed to add member directly: {e}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "message": "Gagal menambahkan anggota ke event", "code": "DB_ERROR" })),
                );
            }

            (
                StatusCode::OK,
                Json(json!({
                    "message": format!("{} (@{}) berhasil ditambahkan sebagai {}", target_name, target_username, role_enum.as_str()),
                    "status": "added",
                    "userId": target_user_id,
                    "role": role_enum.as_str()
                })),
            )
        }
        Ok(None) => {
            // User does not exist yet: create invitation token
            let token = Uuid::new_v4().to_string();
            let expires_at = chrono::Utc::now() + chrono::Duration::days(7);
            let invite_sql = r#"
                INSERT INTO convena.event_invitations (
                    id, event_id, invited_by, email, role, custom_title, token, status, expires_at, created_at
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, 'pending', $8, NOW()
                )
            "#;

            if let Err(e) = db::query(invite_sql)
                .bind(Uuid::new_v4())
                .bind(event_id)
                .bind(current_user_id)
                .bind(&email)
                .bind(role_enum.as_str())
                .bind(&payload.custom_title)
                .bind(&token)
                .bind(expires_at)
                .execute(&state.pool)
                .await
            {
                tracing::error!("Failed to create invitation: {e}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "message": "Gagal membuat tautan undangan", "code": "DB_ERROR" })),
                );
            }

            (
                StatusCode::CREATED,
                Json(json!({
                    "message": format!("Undangan berhasil dibuat untuk {}", email),
                    "status": "invited",
                    "email": email,
                    "role": role_enum.as_str(),
                    "token": token
                })),
            )
        }
        Err(e) => {
            tracing::error!("Failed to lookup user for invite: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan basis data", "code": "DB_ERROR" })),
            )
        }
    }
}

/// Update member role or custom title. Requires TeamManageRoles permission.
pub async fn update_member_handler(
    State(state): State<AppState>,
    Path((event_id, target_user_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<UpdateMemberRolePayload>,
) -> impl IntoResponse {
    let current_user_id = match extract_user_id_from_headers(&headers, &state.config.jwt_secret) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Otorisasi diperlukan", "code": "UNAUTHORIZED" })),
            );
        }
    };

    let can_manage = match check_event_permission(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::TeamManageRoles,
    )
    .await
    {
        Ok(allowed) => allowed,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan otorisasi", "code": "AUTH_ERROR" })),
            );
        }
    };

    if !can_manage {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Anda tidak memiliki izin mengubah role anggota", "code": "FORBIDDEN" })),
        );
    }

    let current_target_role = match get_user_event_role(&state.pool, target_user_id, event_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "message": "Anggota tidak ditemukan di event ini", "code": "MEMBER_NOT_FOUND" })),
            );
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan basis data", "code": "DB_ERROR" })),
            );
        }
    };

    if current_target_role == EventRole::Owner {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Tidak dapat mengubah role Owner event", "code": "CANNOT_MODIFY_OWNER" })),
        );
    }

    let new_role = match EventRole::from_str(&payload.role) {
        Some(r) => r,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": "Role baru tidak valid", "code": "INVALID_ROLE" })),
            );
        }
    };

    let sql = r#"
        UPDATE convena.event_members
        SET role = $1, custom_title = COALESCE($2, custom_title), updated_at = NOW()
        WHERE event_id = $3 AND user_id = $4
    "#;

    if let Err(e) = db::query(sql)
        .bind(new_role.as_str())
        .bind(payload.custom_title)
        .bind(event_id)
        .bind(target_user_id)
        .execute(&state.pool)
        .await
    {
        tracing::error!("Failed to update member role: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "message": "Gagal memperbarui role anggota", "code": "DB_ERROR" })),
        );
    }

    (
        StatusCode::OK,
        Json(json!({
            "message": "Role anggota berhasil diperbarui",
            "role": new_role.as_str()
        })),
    )
}

/// Remove a member from the event. Requires TeamManageRoles permission.
pub async fn remove_member_handler(
    State(state): State<AppState>,
    Path((event_id, target_user_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let current_user_id = match extract_user_id_from_headers(&headers, &state.config.jwt_secret) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Otorisasi diperlukan", "code": "UNAUTHORIZED" })),
            );
        }
    };

    let can_manage = match check_event_permission(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::TeamManageRoles,
    )
    .await
    {
        Ok(allowed) => allowed,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan otorisasi", "code": "AUTH_ERROR" })),
            );
        }
    };

    if !can_manage {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Anda tidak memiliki izin menghapus anggota", "code": "FORBIDDEN" })),
        );
    }

    let target_role = match get_user_event_role(&state.pool, target_user_id, event_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "message": "Anggota tidak ditemukan di event ini", "code": "MEMBER_NOT_FOUND" })),
            );
        }
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan basis data", "code": "DB_ERROR" })),
            );
        }
    };

    if target_role == EventRole::Owner {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Tidak dapat menghapus Owner event", "code": "CANNOT_REMOVE_OWNER" })),
        );
    }

    let sql = "DELETE FROM convena.event_members WHERE event_id = $1 AND user_id = $2";
    if let Err(e) = db::query(sql)
        .bind(event_id)
        .bind(target_user_id)
        .execute(&state.pool)
        .await
    {
        tracing::error!("Failed to delete member: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "message": "Gagal menghapus anggota dari event", "code": "DB_ERROR" })),
        );
    }

    (
        StatusCode::OK,
        Json(json!({ "message": "Anggota berhasil dihapus dari event" })),
    )
}
