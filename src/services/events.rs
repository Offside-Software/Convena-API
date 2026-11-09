use std::{convert::Infallible, time::Duration};

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse,
    },
    routing::{get, patch, post},
    Json, Router,
};
use futures_util::stream::{self, Stream};
use serde_json::json;
use uuid::Uuid;

use crate::{
    appstate::AppState,
    db,
    models::{
        event::{
            AttendEventQrPayload, AttendanceRealtimeEvent, CreateEventPayload,
            CreateEventTaskPayload, Event, EventRegistration, EventTask, EventWithRole,
            RegisterEventPayload, TicketTier, UpdateEventPayload, UpdateEventTaskPayload,
        },
        finance::DEFAULT_BUDGET_CATEGORIES,
        membership::{
            EventMemberDetail, EventPermission, EventRole, InviteMemberPayload,
            UpdateMemberRolePayload,
        },
    },
    services::{
        auth::extract_user_id_from_headers,
        rbac::{check_event_permission, check_org_permission, get_user_event_role},
    },
};

pub fn event_routes() -> Router<AppState> {
    Router::new()
        .route("/", post(create_event_handler).get(list_events_handler))
        .route("/my-events", get(list_my_events_handler))
        .route("/my-registrations", get(list_my_registrations_handler))
        .route("/attend", post(attend_event_by_qr_handler))
        .route("/{id}", get(get_event_handler).patch(update_event_handler))
        .route(
            "/{id}/members",
            get(list_members_handler).post(invite_member_handler),
        )
        .route(
            "/{id}/members/{user_id}",
            patch(update_member_handler).delete(remove_member_handler),
        )
        .route(
            "/{id}/tasks",
            get(list_event_tasks_handler).post(create_event_task_handler),
        )
        .route(
            "/{id}/tasks/{task_id}",
            patch(update_event_task_handler).delete(delete_event_task_handler),
        )
        .route("/{id}/register", post(register_event_handler))
        .route("/{id}/my-registration", get(get_my_event_registration_handler))
        .route("/{id}/registrations", get(list_event_registrations_handler))
        .route(
            "/{id}/registrations/{reg_id}/check-in",
            post(check_in_registration_handler),
        )
        .route("/{id}/attendance-qr", post(generate_attendance_qr_handler))
        .route("/{id}/attendance-stream", get(attendance_stream_handler))
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

pub(crate) fn parse_datetime(s: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M"))
                .or_else(|_| {
                    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
                        .map(|d| d.and_hms_opt(0, 0, 0).unwrap())
                })
                .map(|naive| naive.and_utc())
                .ok()
        })
}

/// Create a new event under an organization, automatically make the creator the 'owner', and initialize RAB categories.
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
                Json(
                    json!({ "message": "Otorisasi diperlukan untuk membuat event", "code": "UNAUTHORIZED" }),
                ),
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

    let start_date = match parse_datetime(&payload.start_date) {
        Some(dt) => dt,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    json!({ "message": "Format tanggal mulai tidak valid", "code": "INVALID_START_DATE" }),
                ),
            );
        }
    };

    let end_date = match parse_datetime(&payload.end_date) {
        Some(dt) => dt,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(
                    json!({ "message": "Format tanggal selesai tidak valid", "code": "INVALID_END_DATE" }),
                ),
            );
        }
    };

    // Resolve organization_id: if provided, verify user has permission in that organization;
    // otherwise auto-select the user's primary organization where they can create events.
    let mut resolved_org_id = payload.organization_id;
    let mut resolved_org_name = payload.organization_name.clone();
    let mut resolved_company = payload.company.clone();

    if let Some(org_id) = resolved_org_id {
        let can_create = check_org_permission(&state.pool, current_user_id, org_id, "event:create")
            .await
            .unwrap_or(false)
            || check_org_permission(&state.pool, current_user_id, org_id, "event:edit")
                .await
                .unwrap_or(false);

        if !can_create {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "message": "Anda tidak memiliki izin membuat event di organisasi ini",
                    "code": "FORBIDDEN"
                })),
            );
        }

        if let Ok(Some((org_name, org_company))) = db::query_as::<sqlx::Postgres, (String, Option<String>)>(
            "SELECT name, company FROM convena.organizations WHERE id = $1",
        )
        .bind(org_id)
        .fetch_optional(&state.pool)
        .await
        {
            if resolved_org_name.as_deref().unwrap_or("").trim().is_empty() {
                resolved_org_name = Some(org_name);
            }
            if resolved_company.as_deref().unwrap_or("").trim().is_empty() {
                resolved_company = org_company;
            }
        }
    } else {
        // Look up user's first active organization membership
        if let Ok(Some((org_id, org_name, org_company))) =
            db::query_as::<sqlx::Postgres, (Uuid, String, Option<String>)>(
                r#"
                SELECT o.id, o.name, o.company
                FROM convena.organization_members om
                INNER JOIN convena.organizations o ON om.organization_id = o.id
                WHERE om.user_id = $1 AND om.status = 'active'
                ORDER BY om.joined_at ASC
                LIMIT 1
                "#,
            )
            .bind(current_user_id)
            .fetch_optional(&state.pool)
            .await
        {
            resolved_org_id = Some(org_id);
            if resolved_org_name.as_deref().unwrap_or("").trim().is_empty() {
                resolved_org_name = Some(org_name);
            }
            if resolved_company.as_deref().unwrap_or("").trim().is_empty() {
                resolved_company = org_company;
            }
        }
    }

    let slug = payload
        .slug
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| generate_slug(&title));

    let event_id = Uuid::new_v4();
    let now = chrono::Utc::now();
    let status = payload.status.unwrap_or_else(|| "draft".to_string());
    let visibility = payload.visibility.unwrap_or_else(|| "public".to_string());
    let location_type = payload
        .location_type
        .unwrap_or_else(|| "offline".to_string());
    let ticket_tiers = sqlx::types::Json(payload.ticket_tiers.unwrap_or_default());
    let attendance_qr_token = format!(
        "CVN-{}-{}",
        &event_id.to_string()[..6].to_uppercase(),
        &Uuid::new_v4().to_string()[..6].to_uppercase()
    );

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
            id, created_by, organization_id, title, slug, description, short_description, banner_url,
            category, status, visibility, start_date, end_date, location_type,
            venue_name, address, city, online_url, latitude, longitude, attendance_qr_token,
            organization_name, company, ticket_tiers,
            created_at, updated_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8,
            $9, $10, $11, $12, $13, $14,
            $15, $16, $17, $18, $19, $20, $21,
            $22, $23, $24,
            $25, $25
        ) RETURNING id, created_by, organization_id, title, slug, description, short_description, banner_url,
                    category, status, visibility, start_date, end_date, location_type,
                    venue_name, address, city, online_url, latitude, longitude, attendance_qr_token,
                    organization_name, company, ticket_tiers,
                    created_at, updated_at
    "#;

    let event = match db::query_as::<sqlx::Postgres, Event>(insert_event_sql)
        .bind(event_id)
        .bind(current_user_id)
        .bind(resolved_org_id)
        .bind(&title)
        .bind(&slug)
        .bind(payload.description)
        .bind(payload.short_description)
        .bind(payload.banner_url)
        .bind(payload.category)
        .bind(status)
        .bind(visibility)
        .bind(start_date)
        .bind(end_date)
        .bind(location_type)
        .bind(payload.venue_name)
        .bind(payload.address)
        .bind(payload.city)
        .bind(payload.online_url)
        .bind(payload.latitude)
        .bind(payload.longitude)
        .bind(&attendance_qr_token)
        .bind(resolved_org_name)
        .bind(resolved_company)
        .bind(ticket_tiers)
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
                Json(
                    json!({ "message": "Gagal menyimpan event ke basis data", "code": "DB_ERROR" }),
                ),
            );
        }
    };

    // 2. Automatically assign creator as 'owner' in convena.event_members
    let member_id = Uuid::new_v4();
    let insert_member_sql = r#"
        INSERT INTO convena.event_members (
            id, event_id, user_id, role, custom_title, division, status, joined_at, updated_at
        ) VALUES (
            $1, $2, $3, 'owner', 'Ketua / Creator', 'BPH / Inti', 'active', $4, $4
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
            Json(
                json!({ "message": "Gagal mengalokasikan role pemilik event", "code": "DB_ERROR" }),
            ),
        );
    }

    // 3. Initialize default RAB budget categories for this event
    let insert_cat_sql = r#"
        INSERT INTO convena.event_budget_categories (
            id, event_id, category, label, planned_amount, created_at, updated_at
        ) VALUES ($1, $2, $3, $4, 0, $5, $5)
        ON CONFLICT (event_id, category) DO NOTHING
    "#;

    for (cat, label) in DEFAULT_BUDGET_CATEGORIES {
        if let Err(e) = db::query(insert_cat_sql)
            .bind(Uuid::new_v4())
            .bind(event_id)
            .bind(*cat)
            .bind(*label)
            .bind(now)
            .execute(&mut *tx)
            .await
        {
            tracing::warn!("Failed to initialize budget category {cat} for event {event_id}: {e}");
        }
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

/// Update an existing event (works for both draft and published events for urgent revisions).
pub async fn update_event_handler(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<UpdateEventPayload>,
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

    let can_edit = match check_event_permission(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::EventEditSettings,
    )
    .await
    {
        Ok(allowed) => allowed,
        Err(e) => {
            tracing::error!("Failed to check permissions for event update: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan otorisasi", "code": "AUTH_ERROR" })),
            );
        }
    };

    if !can_edit {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "message": "Anda tidak memiliki izin mengubah pengaturan event ini",
                "code": "FORBIDDEN"
            })),
        );
    }

    let existing_sql = r#"
        SELECT
            id, created_by, organization_id, title, slug, description, short_description, banner_url,
            category, status, visibility, start_date, end_date, location_type,
            venue_name, address, city, online_url, latitude, longitude, attendance_qr_token,
            organization_name, company, ticket_tiers, created_at, updated_at
        FROM convena.events
        WHERE id = $1
    "#;

    let existing = match db::query_as::<sqlx::Postgres, Event>(existing_sql)
        .bind(event_id)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(Some(ev)) => ev,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "message": "Event tidak ditemukan", "code": "EVENT_NOT_FOUND" })),
            );
        }
        Err(e) => {
            tracing::error!("Failed to fetch existing event for update: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan basis data", "code": "DB_ERROR" })),
            );
        }
    };

    let title = payload
        .title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or(existing.title);
    let organization_id = payload.organization_id.or(existing.organization_id);
    let description = payload.description.or(existing.description);
    let short_description = payload.short_description.or(existing.short_description);
    let banner_url = payload.banner_url.or(existing.banner_url);
    let category = payload.category.unwrap_or(existing.category);
    let status = payload.status.unwrap_or(existing.status);
    let visibility = payload.visibility.unwrap_or(existing.visibility);
    let start_date = payload
        .start_date
        .as_deref()
        .and_then(parse_datetime)
        .unwrap_or(existing.start_date);
    let end_date = payload
        .end_date
        .as_deref()
        .and_then(parse_datetime)
        .unwrap_or(existing.end_date);
    let location_type = payload.location_type.unwrap_or(existing.location_type);
    let venue_name = payload.venue_name.or(existing.venue_name);
    let address = payload.address.or(existing.address);
    let city = payload.city.or(existing.city);
    let online_url = payload.online_url.or(existing.online_url);
    let latitude = payload.latitude.or(existing.latitude);
    let longitude = payload.longitude.or(existing.longitude);
    let organization_name = payload.organization_name.or(existing.organization_name);
    let company = payload.company.or(existing.company);
    let ticket_tiers = sqlx::types::Json(
        payload
            .ticket_tiers
            .unwrap_or_else(|| existing.ticket_tiers.0),
    );

    let update_sql = r#"
        UPDATE convena.events
        SET
            organization_id = $1,
            title = $2,
            description = $3,
            short_description = $4,
            banner_url = $5,
            category = $6,
            status = $7,
            visibility = $8,
            start_date = $9,
            end_date = $10,
            location_type = $11,
            venue_name = $12,
            address = $13,
            city = $14,
            online_url = $15,
            latitude = $16,
            longitude = $17,
            organization_name = $18,
            company = $19,
            ticket_tiers = $20,
            updated_at = NOW()
        WHERE id = $21
        RETURNING id, created_by, organization_id, title, slug, description, short_description, banner_url,
                  category, status, visibility, start_date, end_date, location_type,
                  venue_name, address, city, online_url, latitude, longitude, attendance_qr_token,
                  organization_name, company, ticket_tiers, created_at, updated_at
    "#;

    match db::query_as::<sqlx::Postgres, Event>(update_sql)
        .bind(organization_id)
        .bind(title)
        .bind(description)
        .bind(short_description)
        .bind(banner_url)
        .bind(category)
        .bind(status)
        .bind(visibility)
        .bind(start_date)
        .bind(end_date)
        .bind(location_type)
        .bind(venue_name)
        .bind(address)
        .bind(city)
        .bind(online_url)
        .bind(latitude)
        .bind(longitude)
        .bind(organization_name)
        .bind(company)
        .bind(ticket_tiers)
        .bind(event_id)
        .fetch_one(&state.pool)
        .await
    {
        Ok(updated) => (StatusCode::OK, Json(json!({ "event": updated }))),
        Err(e) => {
            tracing::error!("Failed to update event {event_id}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal memperbarui event", "code": "DB_ERROR" })),
            )
        }
    }
}

/// List all events for the current user along with their respective role in each event (including via organization membership).
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
        SELECT DISTINCT ON (e.id)
            e.id, e.created_by, e.organization_id, e.title, e.slug, e.description, e.short_description, e.banner_url,
            e.category, e.status, e.visibility, e.start_date, e.end_date, e.location_type,
            e.venue_name, e.address, e.city, e.online_url, e.latitude, e.longitude, e.attendance_qr_token,
            COALESCE(o.name, e.organization_name) as organization_name,
            COALESCE(o.company, e.company) as company,
            e.ticket_tiers, e.created_at, e.updated_at,
            COALESCE(m.role, CASE
                WHEN r.name = 'owner' THEN 'owner'
                WHEN r.name IN ('admin', 'event_manager') THEN 'admin'
                WHEN r.name = 'treasurer' THEN 'treasurer'
                ELSE 'committee'
            END) as my_role,
            COALESCE(m.custom_title, om.custom_title, r.display_name) as my_custom_title,
            COALESCE(m.division, 'Umum') as my_division
        FROM convena.events e
        LEFT JOIN convena.organizations o ON e.organization_id = o.id
        LEFT JOIN convena.event_members m ON e.id = m.event_id AND m.user_id = $1 AND m.status = 'active'
        LEFT JOIN convena.organization_members om ON e.organization_id = om.organization_id AND om.user_id = $1 AND om.status = 'active'
        LEFT JOIN convena.organization_roles r ON om.role_id = r.id
        WHERE m.user_id IS NOT NULL OR om.user_id IS NOT NULL
        ORDER BY e.id, e.created_at DESC
    "#;

    match db::query_as::<sqlx::Postgres, EventWithRole>(sql)
        .bind(current_user_id)
        .fetch_all(&state.pool)
        .await
    {
        Ok(mut events) => {
            events.sort_by(|a, b| b.created_at.cmp(&a.created_at));
            (StatusCode::OK, Json(json!(events)))
        }
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
            e.id, e.created_by, e.organization_id, e.title, e.slug, e.description, e.short_description, e.banner_url,
            e.category, e.status, e.visibility, e.start_date, e.end_date, e.location_type,
            e.venue_name, e.address, e.city, e.online_url, e.latitude, e.longitude, e.attendance_qr_token,
            COALESCE(o.name, e.organization_name) as organization_name,
            COALESCE(o.company, e.company) as company,
            e.ticket_tiers, e.created_at, e.updated_at
        FROM convena.events e
        LEFT JOIN convena.organizations o ON e.organization_id = o.id
        WHERE e.visibility = 'public'
        ORDER BY e.start_date ASC
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
            e.id, e.created_by, e.organization_id, e.title, e.slug, e.description, e.short_description, e.banner_url,
            e.category, e.status, e.visibility, e.start_date, e.end_date, e.location_type,
            e.venue_name, e.address, e.city, e.online_url, e.latitude, e.longitude, e.attendance_qr_token,
            COALESCE(o.name, e.organization_name) as organization_name,
            COALESCE(o.company, e.company) as company,
            e.ticket_tiers, e.created_at, e.updated_at
        FROM convena.events e
        LEFT JOIN convena.organizations o ON e.organization_id = o.id
        WHERE e.id = $1
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
            m.role, m.custom_title, COALESCE(m.division, 'Umum') as division, m.status, m.joined_at
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

/// Invite or directly add a member to the event with Role and Division per event.
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
                "message": "Hanya Owner atau Admin yang dapat menugaskan anggota tim baru",
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

    let division = payload
        .division
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Divisi Acara")
        .to_string();

    // Check if user exists in convena.users
    let user_lookup = db::query_as::<sqlx::Postgres, (Uuid, String, String)>(
        "SELECT id, display_name, username FROM convena.users WHERE LOWER(email) = $1",
    )
    .bind(&email)
    .fetch_optional(&state.pool)
    .await;

    match user_lookup {
        Ok(Some((target_user_id, target_name, target_username))) => {
            let member_id = Uuid::new_v4();
            let now = chrono::Utc::now();
            let insert_sql = r#"
                INSERT INTO convena.event_members (
                    id, event_id, user_id, role, custom_title, division, status, joined_at, updated_at
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, 'active', $7, $7
                )
                ON CONFLICT (event_id, user_id) 
                DO UPDATE SET
                    role = EXCLUDED.role,
                    custom_title = EXCLUDED.custom_title,
                    division = EXCLUDED.division,
                    updated_at = EXCLUDED.updated_at
            "#;

            if let Err(e) = db::query(insert_sql)
                .bind(member_id)
                .bind(event_id)
                .bind(target_user_id)
                .bind(role_enum.as_str())
                .bind(&payload.custom_title)
                .bind(&division)
                .bind(now)
                .execute(&state.pool)
                .await
            {
                tracing::error!("Failed to add member directly: {e}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(
                        json!({ "message": "Gagal menambahkan anggota ke event", "code": "DB_ERROR" }),
                    ),
                );
            }

            (
                StatusCode::OK,
                Json(json!({
                    "message": format!("{} (@{}) berhasil ditugaskan sebagai {} di {}", target_name, target_username, role_enum.as_str(), division),
                    "status": "added",
                    "userId": target_user_id,
                    "role": role_enum.as_str(),
                    "division": division
                })),
            )
        }
        Ok(None) => {
            let token = Uuid::new_v4().to_string();
            let expires_at = chrono::Utc::now() + chrono::Duration::days(7);
            let invite_sql = r#"
                INSERT INTO convena.event_invitations (
                    id, event_id, invited_by, email, role, custom_title, division, token, status, expires_at, created_at
                ) VALUES (
                    $1, $2, $3, $4, $5, $6, $7, $8, 'pending', $9, NOW()
                )
            "#;

            if let Err(e) = db::query(invite_sql)
                .bind(Uuid::new_v4())
                .bind(event_id)
                .bind(current_user_id)
                .bind(&email)
                .bind(role_enum.as_str())
                .bind(&payload.custom_title)
                .bind(&division)
                .bind(&token)
                .bind(expires_at)
                .execute(&state.pool)
                .await
            {
                tracing::error!("Failed to create invitation: {e}");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(
                        json!({ "message": "Gagal membuat tautan undangan", "code": "DB_ERROR" }),
                    ),
                );
            }

            (
                StatusCode::CREATED,
                Json(json!({
                    "message": format!("Undangan berhasil dibuat untuk {}", email),
                    "status": "invited",
                    "email": email,
                    "role": role_enum.as_str(),
                    "division": division,
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

/// Update member role, custom title, or division per event. Requires TeamManageRoles permission.
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
            Json(
                json!({ "message": "Anda tidak memiliki izin mengubah penugasan anggota", "code": "FORBIDDEN" }),
            ),
        );
    }

    let current_target_role =
        match get_user_event_role(&state.pool, target_user_id, event_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                return (
                    StatusCode::NOT_FOUND,
                    Json(
                        json!({ "message": "Anggota tidak ditemukan di event ini", "code": "MEMBER_NOT_FOUND" }),
                    ),
                );
            }
            Err(_) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "message": "Kesalahan basis data", "code": "DB_ERROR" })),
                );
            }
        };

    if current_target_role == EventRole::Owner && payload.role != "owner" {
        return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({ "message": "Tidak dapat mengubah role Owner event", "code": "CANNOT_MODIFY_OWNER" }),
            ),
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
        SET
            role = $1,
            custom_title = COALESCE($2, custom_title),
            division = COALESCE($3, division),
            updated_at = NOW()
        WHERE event_id = $4 AND user_id = $5
    "#;

    if let Err(e) = db::query(sql)
        .bind(new_role.as_str())
        .bind(payload.custom_title)
        .bind(payload.division)
        .bind(event_id)
        .bind(target_user_id)
        .execute(&state.pool)
        .await
    {
        tracing::error!("Failed to update member role/division: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "message": "Gagal memperbarui penugasan anggota", "code": "DB_ERROR" })),
        );
    }

    (
        StatusCode::OK,
        Json(json!({
            "message": "Penugasan anggota berhasil diperbarui",
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
            Json(
                json!({ "message": "Anda tidak memiliki izin menghapus anggota", "code": "FORBIDDEN" }),
            ),
        );
    }

    let target_role = match get_user_event_role(&state.pool, target_user_id, event_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(
                    json!({ "message": "Anggota tidak ditemukan di event ini", "code": "MEMBER_NOT_FOUND" }),
                ),
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
            Json(
                json!({ "message": "Tidak dapat menghapus Owner event", "code": "CANNOT_REMOVE_OWNER" }),
            ),
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
            Json(
                json!({ "message": "Gagal menghapus anggota dari event", "code": "DB_ERROR" }),
            ),
        );
    }

    (
        StatusCode::OK,
        Json(json!({ "message": "Anggota berhasil dihapus dari event" })),
    )
}

// ============================================================================
// Per-Event Committee Tasks Management
// ============================================================================

pub async fn list_event_tasks_handler(
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

    let can_view = check_event_permission(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::TeamViewMembers,
    )
    .await
    .unwrap_or(false);

    if !can_view {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Anda tidak memiliki akses ke tugas kepanitiaan event ini", "code": "FORBIDDEN" })),
        );
    }

    let sql = r#"
        SELECT id, event_id, title, description, assignee_id, assignee_name, division, due_date, status, created_at, updated_at
        FROM convena.event_tasks
        WHERE event_id = $1
        ORDER BY created_at DESC
    "#;

    match db::query_as::<sqlx::Postgres, EventTask>(sql)
        .bind(event_id)
        .fetch_all(&state.pool)
        .await
    {
        Ok(tasks) => (StatusCode::OK, Json(json!(tasks))),
        Err(e) => {
            tracing::error!("Failed to list event tasks: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal memuat daftar tugas panitia", "code": "DB_ERROR" })),
            )
        }
    }
}

pub async fn create_event_task_handler(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<CreateEventTaskPayload>,
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

    let can_access = check_event_permission(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::TeamViewMembers,
    )
    .await
    .unwrap_or(false);

    if !can_access {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Tidak memiliki izin membuat task di event ini", "code": "FORBIDDEN" })),
        );
    }

    let title = payload.title.trim().to_string();
    if title.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Judul tugas wajib diisi", "code": "MISSING_TITLE" })),
        );
    }

    let sql = r#"
        INSERT INTO convena.event_tasks (
            id, event_id, title, description, assignee_id, assignee_name, division, due_date, status
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        RETURNING id, event_id, title, description, assignee_id, assignee_name, division, due_date, status, created_at, updated_at
    "#;

    match db::query_as::<sqlx::Postgres, EventTask>(sql)
        .bind(Uuid::new_v4())
        .bind(event_id)
        .bind(title)
        .bind(payload.description.unwrap_or_default())
        .bind(payload.assignee_id)
        .bind(payload.assignee_name.trim())
        .bind(payload.division.trim())
        .bind(payload.due_date.trim())
        .bind(payload.status.unwrap_or_else(|| "todo".to_string()))
        .fetch_one(&state.pool)
        .await
    {
        Ok(task) => (StatusCode::CREATED, Json(json!(task))),
        Err(e) => {
            tracing::error!("Failed to create event task: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal menyimpan tugas baru", "code": "DB_ERROR" })),
            )
        }
    }
}

pub async fn update_event_task_handler(
    State(state): State<AppState>,
    Path((event_id, task_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<UpdateEventTaskPayload>,
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

    let can_access = check_event_permission(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::TeamViewMembers,
    )
    .await
    .unwrap_or(false);

    if !can_access {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Tidak memiliki izin memperbarui task", "code": "FORBIDDEN" })),
        );
    }

    let sql = r#"
        UPDATE convena.event_tasks
        SET
            title = COALESCE($1, title),
            description = COALESCE($2, description),
            assignee_id = COALESCE($3, assignee_id),
            assignee_name = COALESCE($4, assignee_name),
            division = COALESCE($5, division),
            due_date = COALESCE($6, due_date),
            status = COALESCE($7, status),
            updated_at = NOW()
        WHERE id = $8 AND event_id = $9
        RETURNING id, event_id, title, description, assignee_id, assignee_name, division, due_date, status, created_at, updated_at
    "#;

    match db::query_as::<sqlx::Postgres, EventTask>(sql)
        .bind(payload.title)
        .bind(payload.description)
        .bind(payload.assignee_id)
        .bind(payload.assignee_name)
        .bind(payload.division)
        .bind(payload.due_date)
        .bind(payload.status)
        .bind(task_id)
        .bind(event_id)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(Some(task)) => (StatusCode::OK, Json(json!(task))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": "Task tidak ditemukan", "code": "TASK_NOT_FOUND" })),
        ),
        Err(e) => {
            tracing::error!("Failed to update event task: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal memperbarui task", "code": "DB_ERROR" })),
            )
        }
    }
}

pub async fn delete_event_task_handler(
    State(state): State<AppState>,
    Path((event_id, task_id)): Path<(Uuid, Uuid)>,
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

    let can_access = check_event_permission(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::TeamViewMembers,
    )
    .await
    .unwrap_or(false);

    if !can_access {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Tidak memiliki izin menghapus task", "code": "FORBIDDEN" })),
        );
    }

    let sql = "DELETE FROM convena.event_tasks WHERE id = $1 AND event_id = $2";
    match db::query(sql)
        .bind(task_id)
        .bind(event_id)
        .execute(&state.pool)
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({ "message": "Task berhasil dihapus" })),
        ),
        Err(e) => {
            tracing::error!("Failed to delete event task: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal menghapus task", "code": "DB_ERROR" })),
            )
        }
    }
}

// ============================================================================
// Ticket Registration (Free & Paid) + Attendance QR Generation & Check-in
// ============================================================================

/// Register the current user for an event ticket tier (Free Tier -> immediate confirmed; Paid Tier -> QRIS/Bank confirmed).
pub async fn register_event_handler(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<RegisterEventPayload>,
) -> impl IntoResponse {
    let current_user_id = match extract_user_id_from_headers(&headers, &state.config.jwt_secret) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({ "message": "Silakan login untuk mendaftar event", "code": "UNAUTHORIZED" })),
            );
        }
    };

    // Check if user already registered for this event
    let existing_reg = db::query_as::<sqlx::Postgres, EventRegistration>(
        r#"
        SELECT id, event_id, user_id, tier_id, tier_name, price, payment_method, payment_status,
               status, attendee_name, attendee_email, attendee_phone, attendee_institution,
               ticket_code, checked_in_at, created_at
        FROM convena.event_registrations
        WHERE event_id = $1 AND user_id = $2
        "#,
    )
    .bind(event_id)
    .bind(current_user_id)
    .fetch_optional(&state.pool)
    .await;

    if let Ok(Some(reg)) = existing_reg {
        return (
            StatusCode::OK,
            Json(json!({
                "message": "Anda sudah terdaftar di event ini",
                "registration": reg
            })),
        );
    }

    // Fetch event & ticket tiers
    let event_row = db::query_as::<sqlx::Postgres, (String, sqlx::types::Json<Vec<TicketTier>>)>(
        "SELECT title, ticket_tiers FROM convena.events WHERE id = $1",
    )
    .bind(event_id)
    .fetch_optional(&state.pool)
    .await;

    let (event_title, mut tiers_json) = match event_row {
        Ok(Some(row)) => row,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "message": "Event tidak ditemukan", "code": "EVENT_NOT_FOUND" })),
            );
        }
        Err(e) => {
            tracing::error!("Failed to fetch event for registration: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan basis data", "code": "DB_ERROR" })),
            );
        }
    };

    // Find selected tier (or fallback to first tier if empty)
    let tier_idx = tiers_json
        .0
        .iter()
        .position(|t| t.id == payload.tier_id)
        .or_else(|| if !tiers_json.0.is_empty() { Some(0) } else { None });

    let (tier_id, tier_name, price) = match tier_idx {
        Some(idx) => {
            let tier = &mut tiers_json.0[idx];
            if !tier.is_available || (tier.quota > 0 && tier.sold >= tier.quota) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "message": "Kuota kategori tiket ini sudah habis", "code": "QUOTA_FULL" })),
                );
            }
            tier.sold += 1;
            if tier.quota > 0 && tier.sold >= tier.quota {
                tier.is_available = false;
            }
            (tier.id.clone(), tier.name.clone(), tier.price)
        }
        None => (
            "tkt-free".to_string(),
            "Free General Admission".to_string(),
            0.0,
        ),
    };

    // Fetch user info as default attendee profile
    let user_row = db::query_as::<sqlx::Postgres, (String, String, Option<String>)>(
        "SELECT display_name, email, company FROM convena.users WHERE id = $1",
    )
    .bind(current_user_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| ("Peserta Convena".to_string(), "user@convena.id".to_string(), None));

    let attendee_name = payload
        .attendee_name
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(user_row.0);
    let attendee_email = payload
        .attendee_email
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(user_row.1);
    let attendee_institution = payload
        .attendee_institution
        .filter(|s| !s.trim().is_empty())
        .or(user_row.2);

    let payment_method = if price <= 0.0 {
        "free".to_string()
    } else {
        payload
            .payment_method
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| "qris".to_string())
    };

    let ticket_code = format!(
        "TKT-{}-{}",
        &event_id.to_string()[..4].to_uppercase(),
        &Uuid::new_v4().to_string()[..6].to_uppercase()
    );

    let insert_reg_sql = r#"
        INSERT INTO convena.event_registrations (
            id, event_id, user_id, tier_id, tier_name, price,
            payment_method, payment_status, status,
            attendee_name, attendee_email, attendee_phone, attendee_institution,
            ticket_code
        ) VALUES (
            $1, $2, $3, $4, $5, $6,
            $7, 'paid', 'confirmed',
            $8, $9, $10, $11,
            $12
        )
        RETURNING id, event_id, user_id, tier_id, tier_name, price, payment_method, payment_status,
                  status, attendee_name, attendee_email, attendee_phone, attendee_institution,
                  ticket_code, checked_in_at, created_at
    "#;

    let registration = match db::query_as::<sqlx::Postgres, EventRegistration>(insert_reg_sql)
        .bind(Uuid::new_v4())
        .bind(event_id)
        .bind(current_user_id)
        .bind(&tier_id)
        .bind(&tier_name)
        .bind(price)
        .bind(&payment_method)
        .bind(&attendee_name)
        .bind(&attendee_email)
        .bind(&payload.attendee_phone)
        .bind(&attendee_institution)
        .bind(&ticket_code)
        .fetch_one(&state.pool)
        .await
    {
        Ok(reg) => reg,
        Err(e) => {
            tracing::error!("Failed to insert event registration: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal menyimpan pendaftaran tiket", "code": "DB_ERROR" })),
            );
        }
    };

    // Update sold count on convena.events
    let _ = db::query("UPDATE convena.events SET ticket_tiers = $1, updated_at = NOW() WHERE id = $2")
        .bind(tiers_json)
        .bind(event_id)
        .execute(&state.pool)
        .await;

    let _ = state.attendance_tx.send(AttendanceRealtimeEvent {
        event_type: "registration.created".to_string(),
        event_id,
        event_title: event_title.clone(),
        already_checked_in: false,
        message: format!(
            "Peserta baru terdaftar: {} ({})",
            registration.attendee_name, registration.tier_name
        ),
        registration: registration.clone(),
        timestamp: chrono::Utc::now(),
    });

    (
        StatusCode::CREATED,
        Json(json!({
            "message": if price <= 0.0 {
                "Pendaftaran tiket gratis berhasil!"
            } else {
                "Pembayaran terverifikasi & pendaftaran tiket berhasil!"
            },
            "registration": registration
        })),
    )
}

/// Get current user's registration for a specific event.
pub async fn get_my_event_registration_handler(
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

    let sql = r#"
        SELECT id, event_id, user_id, tier_id, tier_name, price, payment_method, payment_status,
               status, attendee_name, attendee_email, attendee_phone, attendee_institution,
               ticket_code, checked_in_at, created_at
        FROM convena.event_registrations
        WHERE event_id = $1 AND user_id = $2
    "#;

    match db::query_as::<sqlx::Postgres, EventRegistration>(sql)
        .bind(event_id)
        .bind(current_user_id)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(reg) => (StatusCode::OK, Json(json!({ "registration": reg }))),
        Err(e) => {
            tracing::error!("Failed to fetch my registration: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan basis data", "code": "DB_ERROR" })),
            )
        }
    }
}

/// List all registrations for the current authenticated user across all events.
pub async fn list_my_registrations_handler(
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
        SELECT id, event_id, user_id, tier_id, tier_name, price, payment_method, payment_status,
               status, attendee_name, attendee_email, attendee_phone, attendee_institution,
               ticket_code, checked_in_at, created_at
        FROM convena.event_registrations
        WHERE user_id = $1
        ORDER BY created_at DESC
    "#;

    match db::query_as::<sqlx::Postgres, EventRegistration>(sql)
        .bind(current_user_id)
        .fetch_all(&state.pool)
        .await
    {
        Ok(regs) => (StatusCode::OK, Json(json!(regs))),
        Err(e) => {
            tracing::error!("Failed to list my registrations: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal memuat daftar tiket Anda", "code": "DB_ERROR" })),
            )
        }
    }
}

/// List all registrations & attendance stats for an event (for event organizers).
pub async fn list_event_registrations_handler(
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

    let can_view = check_event_permission(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::TicketsScanCheckin,
    )
    .await
    .unwrap_or(false)
        || check_event_permission(
            &state.pool,
            current_user_id,
            event_id,
            EventPermission::TeamViewMembers,
        )
        .await
        .unwrap_or(false);

    if !can_view {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Tidak memiliki akses melihat daftar peserta event ini", "code": "FORBIDDEN" })),
        );
    }

    let sql = r#"
        SELECT id, event_id, user_id, tier_id, tier_name, price, payment_method, payment_status,
               status, attendee_name, attendee_email, attendee_phone, attendee_institution,
               ticket_code, checked_in_at, created_at
        FROM convena.event_registrations
        WHERE event_id = $1
        ORDER BY created_at DESC
    "#;

    match db::query_as::<sqlx::Postgres, EventRegistration>(sql)
        .bind(event_id)
        .fetch_all(&state.pool)
        .await
    {
        Ok(regs) => (StatusCode::OK, Json(json!(regs))),
        Err(e) => {
            tracing::error!("Failed to list event registrations: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal memuat daftar peserta", "code": "DB_ERROR" })),
            )
        }
    }
}

/// Generate or rotate the official Event Attendance QR token so registered & paid attendees can scan it.
pub async fn generate_attendance_qr_handler(
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

    let can_generate = check_event_permission(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::TicketsScanCheckin,
    )
    .await
    .unwrap_or(false)
        || check_event_permission(
            &state.pool,
            current_user_id,
            event_id,
            EventPermission::EventEditSettings,
        )
        .await
        .unwrap_or(false);

    if !can_generate {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "message": "Hanya penyelenggara/panitia yang dapat membuat QR Presensi event",
                "code": "FORBIDDEN"
            })),
        );
    }

    let new_token = format!(
        "CVN-{}-{}",
        &event_id.to_string()[..6].to_uppercase(),
        &Uuid::new_v4().to_string()[..6].to_uppercase()
    );

    let sql = r#"
        UPDATE convena.events
        SET attendance_qr_token = $1, updated_at = NOW()
        WHERE id = $2
        RETURNING title, attendance_qr_token
    "#;

    match db::query_as::<sqlx::Postgres, (String, Option<String>)>(sql)
        .bind(&new_token)
        .bind(event_id)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(Some((title, token_opt))) => {
            let token = token_opt.unwrap_or(new_token);
            let qr_payload = format!("CONVENA-ATTEND:{event_id}:{token}");
            (
                StatusCode::OK,
                Json(json!({
                    "eventId": event_id,
                    "eventTitle": title,
                    "qrToken": token,
                    "qrPayload": qr_payload,
                    "message": "QR Presensi berhasil dibuat"
                })),
            )
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": "Event tidak ditemukan", "code": "EVENT_NOT_FOUND" })),
        ),
        Err(e) => {
            tracing::error!("Failed to generate attendance QR: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal membuat QR Presensi", "code": "DB_ERROR" })),
            )
        }
    }
}

/// Verify attendance when a registered user scans the Event Attendance QR code.
/// Performs robust checks:
/// 1. Validates QR code format and matches the active `attendance_qr_token` on the event.
/// 2. Checks that the user is registered for that event in `convena.event_registrations`.
/// 3. Checks that the user's ticket is paid (`payment_status == 'paid'`) and confirmed.
/// 4. Marks the registration as `checked_in` with timestamp.
pub async fn attend_event_by_qr_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<AttendEventQrPayload>,
) -> impl IntoResponse {
    let current_user_id = match extract_user_id_from_headers(&headers, &state.config.jwt_secret) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "message": "Silakan login terlebih dahulu untuk melakukan presensi",
                    "code": "UNAUTHORIZED"
                })),
            );
        }
    };

    let raw = payload.qr_code.trim();
    if raw.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Kode QR Presensi tidak boleh kosong", "code": "EMPTY_QR" })),
        );
    }

    // Parse either "CONVENA-ATTEND:<event_uuid>:<token>" or raw "<token>" with optional eventId
    let (parsed_event_id, parsed_token) = if let Some(rest) = raw.strip_prefix("CONVENA-ATTEND:") {
        let mut parts = rest.splitn(2, ':');
        let ev_str = parts.next().unwrap_or("");
        let tok_str = parts.next().unwrap_or("");
        match Uuid::parse_str(ev_str) {
            Ok(ev_id) if !tok_str.is_empty() => (Some(ev_id), tok_str.to_string()),
            _ => (payload.event_id, raw.to_string()),
        }
    } else {
        (payload.event_id, raw.to_string())
    };

    // Lookup event by (id + token) or by token alone
    let event_lookup = if let Some(ev_id) = parsed_event_id {
        db::query_as::<sqlx::Postgres, (Uuid, String, Option<String>)>(
            "SELECT id, title, attendance_qr_token FROM convena.events WHERE id = $1",
        )
        .bind(ev_id)
        .fetch_optional(&state.pool)
        .await
    } else {
        db::query_as::<sqlx::Postgres, (Uuid, String, Option<String>)>(
            "SELECT id, title, attendance_qr_token FROM convena.events WHERE attendance_qr_token = $1 LIMIT 1",
        )
        .bind(&parsed_token)
        .fetch_optional(&state.pool)
        .await
    };

    let (event_id, event_title, active_token) = match event_lookup {
        Ok(Some(row)) => row,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "message": "Kode QR Presensi tidak valid atau event tidak ditemukan",
                    "code": "INVALID_QR_EVENT"
                })),
            );
        }
        Err(e) => {
            tracing::error!("Failed to lookup event for attendance QR: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan basis data", "code": "DB_ERROR" })),
            );
        }
    };

    // Check 1: Token must match the event's active attendance_qr_token
    if active_token.as_deref() != Some(parsed_token.as_str()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "message": "Kode QR Presensi sudah kedaluwarsa atau tidak cocok dengan event ini",
                "code": "INVALID_QR_TOKEN"
            })),
        );
    }

    // Check 2: User must be registered for this event
    let reg_lookup = db::query_as::<sqlx::Postgres, EventRegistration>(
        r#"
        SELECT id, event_id, user_id, tier_id, tier_name, price, payment_method, payment_status,
               status, attendee_name, attendee_email, attendee_phone, attendee_institution,
               ticket_code, checked_in_at, created_at
        FROM convena.event_registrations
        WHERE event_id = $1 AND user_id = $2
        "#,
    )
    .bind(event_id)
    .bind(current_user_id)
    .fetch_optional(&state.pool)
    .await;

    let registration = match reg_lookup {
        Ok(Some(reg)) => reg,
        Ok(None) => {
            return (
                StatusCode::FORBIDDEN,
                Json(json!({
                    "message": format!("Anda belum terdaftar pada event \"{}\". Hanya peserta yang sudah mendaftar dan menyelesaikan pembayaran yang dapat melakukan presensi.", event_title),
                    "code": "NOT_REGISTERED"
                })),
            );
        }
        Err(e) => {
            tracing::error!("Failed to lookup registration for attendance: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan basis data", "code": "DB_ERROR" })),
            );
        }
    };

    // Check 3: Payment must be completed & status confirmed/checked_in
    if registration.payment_status != "paid"
        || (registration.status != "confirmed" && registration.status != "checked_in")
    {
        return (
            StatusCode::PAYMENT_REQUIRED,
            Json(json!({
                "message": "Tiket Anda belum lunas atau belum terkonfirmasi. Silakan selesaikan pembayaran terlebih dahulu.",
                "code": "PAYMENT_NOT_COMPLETED"
            })),
        );
    }

    if registration.status == "checked_in" {
        let msg = format!(
            "Peserta {} ({}) memindai QR Presensi (Sudah tercatat hadir sebelumnya).",
            registration.attendee_name, registration.ticket_code
        );
        let _ = state.attendance_tx.send(AttendanceRealtimeEvent {
            event_type: "attendance.checked_in".to_string(),
            event_id,
            event_title: event_title.clone(),
            already_checked_in: true,
            message: msg,
            registration: registration.clone(),
            timestamp: chrono::Utc::now(),
        });

        return (
            StatusCode::OK,
            Json(json!({
                "message": format!("Anda sudah tercatat hadir di event \"{}\" sebelumnya.", event_title),
                "alreadyCheckedIn": true,
                "eventTitle": event_title,
                "registration": registration
            })),
        );
    }

    // Check 4: Mark as checked_in
    let update_reg_sql = r#"
        UPDATE convena.event_registrations
        SET status = 'checked_in', checked_in_at = NOW()
        WHERE id = $1
        RETURNING id, event_id, user_id, tier_id, tier_name, price, payment_method, payment_status,
                  status, attendee_name, attendee_email, attendee_phone, attendee_institution,
                  ticket_code, checked_in_at, created_at
    "#;

    match db::query_as::<sqlx::Postgres, EventRegistration>(update_reg_sql)
        .bind(registration.id)
        .fetch_one(&state.pool)
        .await
    {
        Ok(updated_reg) => {
            let _ = state.attendance_tx.send(AttendanceRealtimeEvent {
                event_type: "attendance.checked_in".to_string(),
                event_id,
                event_title: event_title.clone(),
                already_checked_in: false,
                message: format!(
                    "{} ({}) berhasil melakukan presensi kehadiran!",
                    updated_reg.attendee_name, updated_reg.ticket_code
                ),
                registration: updated_reg.clone(),
                timestamp: chrono::Utc::now(),
            });

            (
                StatusCode::OK,
                Json(json!({
                    "message": format!("Presensi berhasil! Selamat datang di \"{}\".", event_title),
                    "alreadyCheckedIn": false,
                    "eventTitle": event_title,
                    "registration": updated_reg
                })),
            )
        }
        Err(e) => {
            tracing::error!("Failed to mark registration checked_in: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal mencatat presensi kehadiran", "code": "DB_ERROR" })),
            )
        }
    }
}

/// Allow event organizers / committee members to directly check in a registered & paid attendee
/// (or trigger the real-time attendance event from the organizer console).
pub async fn check_in_registration_handler(
    State(state): State<AppState>,
    Path((event_id, reg_id)): Path<(Uuid, Uuid)>,
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

    let can_checkin = check_event_permission(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::TicketsScanCheckin,
    )
    .await
    .unwrap_or(false)
        || check_event_permission(
            &state.pool,
            current_user_id,
            event_id,
            EventPermission::TeamViewMembers,
        )
        .await
        .unwrap_or(false);

    if !can_checkin {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "message": "Hanya penyelenggara/panitia yang dapat memverifikasi kehadiran peserta",
                "code": "FORBIDDEN"
            })),
        );
    }

    let event_title = db::query_scalar::<sqlx::Postgres, String>(
        "SELECT title FROM convena.events WHERE id = $1",
    )
    .bind(event_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| "Event Convena".to_string());

    let update_reg_sql = r#"
        UPDATE convena.event_registrations
        SET status = 'checked_in', checked_in_at = COALESCE(checked_in_at, NOW())
        WHERE id = $1 AND event_id = $2
        RETURNING id, event_id, user_id, tier_id, tier_name, price, payment_method, payment_status,
                  status, attendee_name, attendee_email, attendee_phone, attendee_institution,
                  ticket_code, checked_in_at, created_at
    "#;

    match db::query_as::<sqlx::Postgres, EventRegistration>(update_reg_sql)
        .bind(reg_id)
        .bind(event_id)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(Some(updated_reg)) => {
            let _ = state.attendance_tx.send(AttendanceRealtimeEvent {
                event_type: "attendance.checked_in".to_string(),
                event_id,
                event_title: event_title.clone(),
                already_checked_in: false,
                message: format!(
                    "{} ({}) berhasil melakukan presensi kehadiran!",
                    updated_reg.attendee_name, updated_reg.ticket_code
                ),
                registration: updated_reg.clone(),
                timestamp: chrono::Utc::now(),
            });

            (
                StatusCode::OK,
                Json(json!({
                    "message": format!("Peserta {} berhasil ditandai HADIR.", updated_reg.attendee_name),
                    "alreadyCheckedIn": false,
                    "eventTitle": event_title,
                    "registration": updated_reg
                })),
            )
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": "Data pendaftaran peserta tidak ditemukan", "code": "NOT_FOUND" })),
        ),
        Err(e) => {
            tracing::error!("Failed to check in registration: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal memperbarui status kehadiran peserta", "code": "DB_ERROR" })),
            )
        }
    }
}

/// Real-time Server-Sent Events (SSE) stream for live attendance & registration updates on an event.
pub async fn attendance_stream_handler(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let rx = state.attendance_tx.subscribe();
    let event_stream = stream::unfold(rx, move |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(evt) => {
                    if evt.event_id == event_id {
                        if let Ok(json_str) = serde_json::to_string(&evt) {
                            let sse_evt = SseEvent::default()
                                .event(evt.event_type.clone())
                                .data(json_str);
                            return Some((Ok(sse_evt), rx));
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });

    Sse::new(event_stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}
