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
    models::organization::{
        AddMemberPayload, CreateOrganizationPayload, CreateRolePayload, Organization,
        OrganizationMemberDetail, OrganizationRole, PermissionCategoryGroup, PermissionItem,
        UpdateMemberRolePayload, UpdateOrganizationPayload, UpdateRolePayload,
        DEFAULT_SYSTEM_ROLES, PERMISSIONS_CATALOG,
    },
    services::{
        auth::extract_user_id_from_headers,
        rbac::{check_org_permission, get_user_org_role_and_permissions},
    },
};

pub fn organization_routes() -> Router<AppState> {
    Router::new()
        .route("/", post(create_organization_handler).get(list_organizations_handler))
        .route("/permissions/catalog", get(get_permissions_catalog_handler))
        .route(
            "/{id}",
            get(get_organization_handler)
                .patch(update_organization_handler)
                .delete(delete_organization_handler),
        )
        .route(
            "/{id}/roles",
            get(list_roles_handler).post(create_role_handler),
        )
        .route(
            "/{id}/roles/{role_id}",
            patch(update_role_handler).delete(delete_role_handler),
        )
        .route(
            "/{id}/members",
            get(list_members_handler).post(add_member_handler),
        )
        .route(
            "/{id}/members/{member_id}",
            patch(update_member_handler).delete(remove_member_handler),
        )
}

fn generate_org_slug(name: &str) -> String {
    let clean: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let trimmed = clean.trim_matches('-');
    let suffix = &Uuid::new_v4().to_string()[..6];
    format!("{trimmed}-{suffix}")
}

/// GET /permissions/catalog - Grouped catalog of permissions
pub async fn get_permissions_catalog_handler() -> impl IntoResponse {
    let mut category_map: std::collections::BTreeMap<&'static str, (&'static str, Vec<PermissionItem>)> =
        std::collections::BTreeMap::new();

    category_map.insert("organization", ("Pengaturan Organisasi", Vec::new()));
    category_map.insert("events", ("Manajemen Acara & Event", Vec::new()));
    category_map.insert("finances", ("Keuangan, RAB & SPJ", Vec::new()));
    category_map.insert("sponsorship", ("Kemitraan & Sponsorship", Vec::new()));
    category_map.insert("certificates", ("Sertifikat & Validasi", Vec::new()));

    for (key, cat, display_name, desc) in PERMISSIONS_CATALOG {
        let entry = category_map.entry(cat).or_insert(("Lainnya", Vec::new()));
        entry.1.push(PermissionItem {
            key: key.to_string(),
            category: cat.to_string(),
            display_name: display_name.to_string(),
            description: desc.to_string(),
        });
    }

    let result: Vec<PermissionCategoryGroup> = category_map
        .into_iter()
        .map(|(cat, (label, permissions))| PermissionCategoryGroup {
            category: cat.to_string(),
            label: label.to_string(),
            permissions,
        })
        .collect();

    (StatusCode::OK, Json(json!(result)))
}

/// GET /organizations - List organizations current user is affiliated with
pub async fn list_organizations_handler(
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
            o.id, o.name, o.slug, o.company, o.description, o.logo_url, o.banner_url,
            o.created_by, o.created_at, o.updated_at,
            (SELECT COUNT(*) FROM convena.organization_members om WHERE om.organization_id = o.id AND om.status = 'active') as member_count,
            (SELECT COUNT(*) FROM convena.events ev WHERE ev.organization_id = o.id) as event_count,
            r.name as my_role,
            r.display_name as my_role_display,
            r.permissions as my_permissions
        FROM convena.organizations o
        INNER JOIN convena.organization_members m ON o.id = m.organization_id
        INNER JOIN convena.organization_roles r ON m.role_id = r.id
        WHERE m.user_id = $1 AND m.status = 'active'
        ORDER BY o.name ASC
    "#;

    #[derive(sqlx::FromRow)]
    struct OrgRow {
        id: Uuid,
        name: String,
        slug: String,
        company: Option<String>,
        description: Option<String>,
        logo_url: Option<String>,
        banner_url: Option<String>,
        created_by: Uuid,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
        member_count: Option<i64>,
        event_count: Option<i64>,
        my_role: Option<String>,
        my_role_display: Option<String>,
        my_permissions: Option<sqlx::types::Json<Vec<String>>>,
    }

    match db::query_as::<sqlx::Postgres, OrgRow>(sql)
        .bind(current_user_id)
        .fetch_all(&state.pool)
        .await
    {
        Ok(rows) => {
            let orgs: Vec<Organization> = rows
                .into_iter()
                .map(|r| Organization {
                    id: r.id,
                    name: r.name,
                    slug: r.slug,
                    company: r.company,
                    description: r.description,
                    logo_url: r.logo_url,
                    banner_url: r.banner_url,
                    created_by: r.created_by,
                    created_at: r.created_at,
                    updated_at: r.updated_at,
                    member_count: r.member_count,
                    event_count: r.event_count,
                    my_role: r.my_role,
                    my_role_display: r.my_role_display,
                    my_permissions: r.my_permissions.map(|p| p.0),
                })
                .collect();
            (StatusCode::OK, Json(json!(orgs)))
        }
        Err(e) => {
            tracing::error!("Failed to list organizations: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal memuat daftar organisasi", "code": "DB_ERROR" })),
            )
        }
    }
}

/// POST /organizations - Create new organization and auto-seed default roles + owner member
pub async fn create_organization_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CreateOrganizationPayload>,
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

    let name = payload.name.trim().to_string();
    if name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Nama organisasi wajib diisi", "code": "MISSING_NAME" })),
        );
    }

    let slug = payload
        .slug
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| generate_org_slug(&name));

    let org_id = Uuid::new_v4();
    let now = chrono::Utc::now();

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

    // 1. Insert Organization
    let insert_org_sql = r#"
        INSERT INTO convena.organizations (
            id, name, slug, company, description, logo_url, banner_url, created_by, created_at, updated_at
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $9)
        RETURNING id, name, slug, company, description, logo_url, banner_url, created_by, created_at, updated_at
    "#;

    let org_row = match db::query_as::<sqlx::Postgres, (Uuid, String, String, Option<String>, Option<String>, Option<String>, Option<String>, Uuid, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)>(insert_org_sql)
        .bind(org_id)
        .bind(&name)
        .bind(&slug)
        .bind(payload.company)
        .bind(payload.description)
        .bind(payload.logo_url)
        .bind(payload.banner_url)
        .bind(current_user_id)
        .bind(now)
        .fetch_one(&mut *tx)
        .await
    {
        Ok(row) => row,
        Err(e) => {
            tracing::error!("Failed to insert organization: {e}");
            let _ = tx.rollback().await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal menyimpan organisasi", "code": "DB_ERROR" })),
            );
        }
    };

    // 2. Insert Default Roles
    let insert_role_sql = r#"
        INSERT INTO convena.organization_roles (
            id, organization_id, name, display_name, description, is_system, permissions, created_at, updated_at
        ) VALUES ($1, $2, $3, $4, $5, true, $6, $7, $7)
    "#;

    let mut owner_role_id = Uuid::new_v4();
    for (role_name, display_name, desc, perms) in DEFAULT_SYSTEM_ROLES {
        let rid = Uuid::new_v4();
        if *role_name == "owner" {
            owner_role_id = rid;
        }
        let perms_json = sqlx::types::Json(perms.to_vec());
        if let Err(e) = db::query(insert_role_sql)
            .bind(rid)
            .bind(org_id)
            .bind(*role_name)
            .bind(*display_name)
            .bind(*desc)
            .bind(perms_json)
            .bind(now)
            .execute(&mut *tx)
            .await
        {
            tracing::error!("Failed to insert role {role_name}: {e}");
            let _ = tx.rollback().await;
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal menginisialisasi role organisasi", "code": "DB_ERROR" })),
            );
        }
    }

    // 3. Add Creator as Owner Member
    let insert_member_sql = r#"
        INSERT INTO convena.organization_members (
            id, organization_id, user_id, role_id, custom_title, status, joined_at, updated_at
        ) VALUES ($1, $2, $3, $4, 'Founder / Ketua Umum', 'active', $5, $5)
    "#;

    if let Err(e) = db::query(insert_member_sql)
        .bind(Uuid::new_v4())
        .bind(org_id)
        .bind(current_user_id)
        .bind(owner_role_id)
        .bind(now)
        .execute(&mut *tx)
        .await
    {
        tracing::error!("Failed to insert organization owner member: {e}");
        let _ = tx.rollback().await;
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "message": "Gagal menambahkan pemilik ke organisasi", "code": "DB_ERROR" })),
        );
    }

    if let Err(e) = tx.commit().await {
        tracing::error!("Failed to commit transaction: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "message": "Gagal menyelesaikan transaksi", "code": "DB_ERROR" })),
        );
    }

    let created_org = Organization {
        id: org_row.0,
        name: org_row.1,
        slug: org_row.2,
        company: org_row.3,
        description: org_row.4,
        logo_url: org_row.5,
        banner_url: org_row.6,
        created_by: org_row.7,
        created_at: org_row.8,
        updated_at: org_row.9,
        member_count: Some(1),
        event_count: Some(0),
        my_role: Some("owner".to_string()),
        my_role_display: Some("Owner".to_string()),
        my_permissions: Some(PERMISSIONS_CATALOG.iter().map(|(k, _, _, _)| k.to_string()).collect()),
    };

    (StatusCode::CREATED, Json(json!(created_org)))
}

/// GET /organizations/{id} - Get organization details with permissions
pub async fn get_organization_handler(
    State(state): State<AppState>,
    Path(org_id): Path<Uuid>,
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

    let user_role_perms = match get_user_org_role_and_permissions(&state.pool, current_user_id, org_id).await {
        Ok(val) => val,
        Err(e) => {
            tracing::error!("Failed to query user permissions: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan memeriksa izin", "code": "DB_ERROR" })),
            );
        }
    };

    let sql = r#"
        SELECT 
            o.id, o.name, o.slug, o.company, o.description, o.logo_url, o.banner_url,
            o.created_by, o.created_at, o.updated_at,
            (SELECT COUNT(*) FROM convena.organization_members om WHERE om.organization_id = o.id AND om.status = 'active') as member_count,
            (SELECT COUNT(*) FROM convena.events ev WHERE ev.organization_id = o.id) as event_count
        FROM convena.organizations o
        WHERE o.id = $1
    "#;

    #[derive(sqlx::FromRow)]
    struct OrgDetailRow {
        id: Uuid,
        name: String,
        slug: String,
        company: Option<String>,
        description: Option<String>,
        logo_url: Option<String>,
        banner_url: Option<String>,
        created_by: Uuid,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
        member_count: Option<i64>,
        event_count: Option<i64>,
    }

    match db::query_as::<sqlx::Postgres, OrgDetailRow>(sql)
        .bind(org_id)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(Some(r)) => {
            let (my_role, my_role_display, my_permissions) = match user_role_perms {
                Some((role_name, display_name, perms)) => {
                    let full_perms = if role_name == "owner" {
                        PERMISSIONS_CATALOG.iter().map(|(k, _, _, _)| k.to_string()).collect()
                    } else {
                        perms
                    };
                    (Some(role_name), Some(display_name), Some(full_perms))
                }
                None => (None, None, None),
            };

            let org = Organization {
                id: r.id,
                name: r.name,
                slug: r.slug,
                company: r.company,
                description: r.description,
                logo_url: r.logo_url,
                banner_url: r.banner_url,
                created_by: r.created_by,
                created_at: r.created_at,
                updated_at: r.updated_at,
                member_count: r.member_count,
                event_count: r.event_count,
                my_role,
                my_role_display,
                my_permissions,
            };

            (StatusCode::OK, Json(json!(org)))
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": "Organisasi tidak ditemukan", "code": "ORG_NOT_FOUND" })),
        ),
        Err(e) => {
            tracing::error!("Failed to get organization: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan basis data", "code": "DB_ERROR" })),
            )
        }
    }
}

/// PATCH /organizations/{id} - Update organization profile (requires org:edit)
pub async fn update_organization_handler(
    State(state): State<AppState>,
    Path(org_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<UpdateOrganizationPayload>,
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

    let can_edit = match check_org_permission(&state.pool, current_user_id, org_id, "org:edit").await {
        Ok(allowed) => allowed,
        Err(_) => false,
    };

    if !can_edit {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Anda tidak memiliki izin mengedit organisasi ini", "code": "FORBIDDEN" })),
        );
    }

    let sql = r#"
        UPDATE convena.organizations
        SET 
            name = COALESCE($1, name),
            company = COALESCE($2, company),
            description = COALESCE($3, description),
            logo_url = COALESCE($4, logo_url),
            banner_url = COALESCE($5, banner_url),
            updated_at = NOW()
        WHERE id = $6
        RETURNING id, name, slug, company, description, logo_url, banner_url, created_by, created_at, updated_at
    "#;

    #[derive(sqlx::FromRow)]
    struct UpdatedOrgRow {
        id: Uuid,
        name: String,
        slug: String,
        company: Option<String>,
        description: Option<String>,
        logo_url: Option<String>,
        banner_url: Option<String>,
        created_by: Uuid,
        created_at: chrono::DateTime<chrono::Utc>,
        updated_at: chrono::DateTime<chrono::Utc>,
    }

    match db::query_as::<sqlx::Postgres, UpdatedOrgRow>(sql)
        .bind(payload.name)
        .bind(payload.company)
        .bind(payload.description)
        .bind(payload.logo_url)
        .bind(payload.banner_url)
        .bind(org_id)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(Some(r)) => (
            StatusCode::OK,
            Json(json!({
                "id": r.id,
                "name": r.name,
                "slug": r.slug,
                "company": r.company,
                "description": r.description,
                "logoUrl": r.logo_url,
                "bannerUrl": r.banner_url,
                "createdBy": r.created_by,
                "createdAt": r.created_at,
                "updatedAt": r.updated_at
            })),
        ),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": "Organisasi tidak ditemukan", "code": "ORG_NOT_FOUND" })),
        ),
        Err(e) => {
            tracing::error!("Failed to update organization: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal memperbarui organisasi", "code": "DB_ERROR" })),
            )
        }
    }
}

/// DELETE /organizations/{id} - Delete organization (requires org:delete / owner only)
pub async fn delete_organization_handler(
    State(state): State<AppState>,
    Path(org_id): Path<Uuid>,
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

    let can_delete = match check_org_permission(&state.pool, current_user_id, org_id, "org:delete").await {
        Ok(allowed) => allowed,
        Err(_) => false,
    };

    if !can_delete {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Hanya Owner yang dapat menghapus organisasi", "code": "FORBIDDEN" })),
        );
    }

    match db::query("DELETE FROM convena.organizations WHERE id = $1")
        .bind(org_id)
        .execute(&state.pool)
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({ "message": "Organisasi berhasil dihapus", "success": true })),
        ),
        Err(e) => {
            tracing::error!("Failed to delete organization: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal menghapus organisasi", "code": "DB_ERROR" })),
            )
        }
    }
}

/// GET /organizations/{id}/roles - List all roles for this organization
pub async fn list_roles_handler(
    State(state): State<AppState>,
    Path(org_id): Path<Uuid>,
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

    let can_view = match check_org_permission(&state.pool, current_user_id, org_id, "org:view").await {
        Ok(allowed) => allowed,
        Err(_) => false,
    };

    if !can_view {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Anda tidak memiliki akses melihat role organisasi ini", "code": "FORBIDDEN" })),
        );
    }

    let sql = r#"
        SELECT 
            r.id, r.organization_id, r.name, r.display_name, r.description,
            r.is_system, r.permissions, r.created_at, r.updated_at,
            (SELECT COUNT(*) FROM convena.organization_members om WHERE om.role_id = r.id AND om.status = 'active') as member_count
        FROM convena.organization_roles r
        WHERE r.organization_id = $1
        ORDER BY 
            CASE r.name
                WHEN 'owner' THEN 1
                WHEN 'admin' THEN 2
                WHEN 'treasurer' THEN 3
                WHEN 'event_manager' THEN 4
                WHEN 'member' THEN 5
                ELSE 6
            END,
            r.created_at ASC
    "#;

    match db::query_as::<sqlx::Postgres, OrganizationRole>(sql)
        .bind(org_id)
        .fetch_all(&state.pool)
        .await
    {
        Ok(roles) => (StatusCode::OK, Json(json!(roles))),
        Err(e) => {
            tracing::error!("Failed to list roles: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal memuat daftar role", "code": "DB_ERROR" })),
            )
        }
    }
}

/// POST /organizations/{id}/roles - Create a new custom role
pub async fn create_role_handler(
    State(state): State<AppState>,
    Path(org_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<CreateRolePayload>,
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

    let can_manage = match check_org_permission(&state.pool, current_user_id, org_id, "org:manage_roles").await {
        Ok(allowed) => allowed,
        Err(_) => false,
    };

    if !can_manage {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Anda tidak memiliki izin mengelola role organisasi", "code": "FORBIDDEN" })),
        );
    }

    let raw_name = payload.name.trim().to_lowercase().replace(' ', "_");
    if raw_name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Nama role wajib diisi", "code": "MISSING_ROLE_NAME" })),
        );
    }

    let display_name = payload.display_name.trim().to_string();
    if display_name.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Nama tampilan role wajib diisi", "code": "MISSING_DISPLAY_NAME" })),
        );
    }

    let role_id = Uuid::new_v4();
    let perms_json = sqlx::types::Json(payload.permissions);
    let now = chrono::Utc::now();

    let sql = r#"
        INSERT INTO convena.organization_roles (
            id, organization_id, name, display_name, description, is_system, permissions, created_at, updated_at
        ) VALUES ($1, $2, $3, $4, $5, false, $6, $7, $7)
        RETURNING id, organization_id, name, display_name, description, is_system, permissions, created_at, updated_at
    "#;

    match db::query_as::<sqlx::Postgres, OrganizationRole>(sql)
        .bind(role_id)
        .bind(org_id)
        .bind(&raw_name)
        .bind(&display_name)
        .bind(payload.description)
        .bind(perms_json)
        .bind(now)
        .fetch_one(&state.pool)
        .await
    {
        Ok(mut role) => {
            role.member_count = Some(0);
            (StatusCode::CREATED, Json(json!(role)))
        }
        Err(e) => {
            tracing::error!("Failed to create role: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal menambahkan role baru. Pastikan nama unik.", "code": "DB_ERROR" })),
            )
        }
    }
}

/// PATCH /organizations/{id}/roles/{role_id} - Update role permissions & details
pub async fn update_role_handler(
    State(state): State<AppState>,
    Path((org_id, role_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<UpdateRolePayload>,
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

    let can_manage = match check_org_permission(&state.pool, current_user_id, org_id, "org:manage_roles").await {
        Ok(allowed) => allowed,
        Err(_) => false,
    };

    if !can_manage {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Anda tidak memiliki izin mengelola role organisasi", "code": "FORBIDDEN" })),
        );
    }

    // Check if role is owner - owner cannot have permissions reduced
    let existing_role: Option<(String, bool)> = db::query_as::<sqlx::Postgres, (String, bool)>(
        "SELECT name, is_system FROM convena.organization_roles WHERE id = $1 AND organization_id = $2",
    )
    .bind(role_id)
    .bind(org_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let (role_name, _is_system) = match existing_role {
        Some(r) => r,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "message": "Role tidak ditemukan", "code": "ROLE_NOT_FOUND" })),
            );
        }
    };

    if role_name == "owner" && payload.permissions.is_some() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Hak akses peran Owner bersifat permanen dan tidak dapat dikurangi", "code": "CANNOT_MODIFY_OWNER_PERMS" })),
        );
    }

    let perms_json = payload.permissions.map(sqlx::types::Json);

    let sql = r#"
        UPDATE convena.organization_roles
        SET 
            display_name = COALESCE($1, display_name),
            description = COALESCE($2, description),
            permissions = COALESCE($3, permissions),
            updated_at = NOW()
        WHERE id = $4 AND organization_id = $5
        RETURNING id, organization_id, name, display_name, description, is_system, permissions, created_at, updated_at
    "#;

    match db::query_as::<sqlx::Postgres, OrganizationRole>(sql)
        .bind(payload.display_name)
        .bind(payload.description)
        .bind(perms_json)
        .bind(role_id)
        .bind(org_id)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(Some(role)) => (StatusCode::OK, Json(json!(role))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": "Role tidak ditemukan", "code": "ROLE_NOT_FOUND" })),
        ),
        Err(e) => {
            tracing::error!("Failed to update role: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal memperbarui role", "code": "DB_ERROR" })),
            )
        }
    }
}

/// DELETE /organizations/{id}/roles/{role_id} - Delete custom role
pub async fn delete_role_handler(
    State(state): State<AppState>,
    Path((org_id, role_id)): Path<(Uuid, Uuid)>,
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

    let can_manage = match check_org_permission(&state.pool, current_user_id, org_id, "org:manage_roles").await {
        Ok(allowed) => allowed,
        Err(_) => false,
    };

    if !can_manage {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Anda tidak memiliki izin mengelola role organisasi", "code": "FORBIDDEN" })),
        );
    }

    let role_info: Option<(String, bool)> = db::query_as::<sqlx::Postgres, (String, bool)>(
        "SELECT name, is_system FROM convena.organization_roles WHERE id = $1 AND organization_id = $2",
    )
    .bind(role_id)
    .bind(org_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let (_, is_system) = match role_info {
        Some(r) => r,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "message": "Role tidak ditemukan", "code": "ROLE_NOT_FOUND" })),
            );
        }
    };

    if is_system {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Role bawaan sistem tidak dapat dihapus", "code": "CANNOT_DELETE_SYSTEM_ROLE" })),
        );
    }

    // Check if any active members have this role
    let active_count: i64 = db::query_scalar::<sqlx::Postgres, i64>(
        "SELECT COUNT(*) FROM convena.organization_members WHERE role_id = $1 AND status = 'active'",
    )
    .bind(role_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(0);

    if active_count > 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "message": format!("Terdapat {active_count} anggota yang masih menggunakan role ini. Harap ubah role anggota terlebih dahulu."),
                "code": "ROLE_IN_USE"
            })),
        );
    }

    match db::query("DELETE FROM convena.organization_roles WHERE id = $1 AND organization_id = $2")
        .bind(role_id)
        .bind(org_id)
        .execute(&state.pool)
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({ "message": "Role berhasil dihapus", "success": true })),
        ),
        Err(e) => {
            tracing::error!("Failed to delete role: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal menghapus role", "code": "DB_ERROR" })),
            )
        }
    }
}

/// GET /organizations/{id}/members - List all organization members
pub async fn list_members_handler(
    State(state): State<AppState>,
    Path(org_id): Path<Uuid>,
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

    let can_view = match check_org_permission(&state.pool, current_user_id, org_id, "org:view").await {
        Ok(allowed) => allowed,
        Err(_) => false,
    };

    if !can_view {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Anda tidak memiliki akses melihat anggota organisasi ini", "code": "FORBIDDEN" })),
        );
    }

    let sql = r#"
        SELECT 
            m.id, m.organization_id, m.user_id,
            u.display_name, u.username, u.email, u.company,
            r.id as role_id, r.name as role_name, r.display_name as role_display_name,
            m.custom_title, m.status, m.joined_at, m.updated_at
        FROM convena.organization_members m
        INNER JOIN convena.users u ON m.user_id = u.id
        INNER JOIN convena.organization_roles r ON m.role_id = r.id
        WHERE m.organization_id = $1
        ORDER BY 
            CASE r.name
                WHEN 'owner' THEN 1
                WHEN 'admin' THEN 2
                WHEN 'treasurer' THEN 3
                WHEN 'event_manager' THEN 4
                WHEN 'member' THEN 5
                ELSE 6
            END,
            m.joined_at ASC
    "#;

    match db::query_as::<sqlx::Postgres, OrganizationMemberDetail>(sql)
        .bind(org_id)
        .fetch_all(&state.pool)
        .await
    {
        Ok(members) => (StatusCode::OK, Json(json!(members))),
        Err(e) => {
            tracing::error!("Failed to list organization members: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal memuat anggota organisasi", "code": "DB_ERROR" })),
            )
        }
    }
}

/// POST /organizations/{id}/members - Add member immediately (no invitations flow)
pub async fn add_member_handler(
    State(state): State<AppState>,
    Path(org_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<AddMemberPayload>,
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

    let can_manage = match check_org_permission(&state.pool, current_user_id, org_id, "org:manage_members").await {
        Ok(allowed) => allowed,
        Err(_) => false,
    };

    if !can_manage {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Anda tidak memiliki izin menambah anggota ke organisasi ini", "code": "FORBIDDEN" })),
        );
    }

    let identifier = payload.identifier.trim().to_lowercase();
    if identifier.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Email atau username anggota wajib diisi", "code": "MISSING_IDENTIFIER" })),
        );
    }

    // Verify role exists in this organization
    let role_check: Option<(String,)> = db::query_as::<sqlx::Postgres, (String,)>(
        "SELECT name FROM convena.organization_roles WHERE id = $1 AND organization_id = $2",
    )
    .bind(payload.role_id)
    .bind(org_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let (role_name,) = match role_check {
        Some(r) => r,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": "Role yang dipilih tidak valid untuk organisasi ini", "code": "INVALID_ROLE" })),
            );
        }
    };

    if role_name == "owner" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Tidak dapat menambahkan anggota baru sebagai Owner langsung. Gunakan transfer kepemilikan.", "code": "CANNOT_ADD_OWNER" })),
        );
    }

    // Lookup user in convena.users
    let user_lookup: Option<(Uuid, String, String, String, Option<String>)> = db::query_as::<sqlx::Postgres, (Uuid, String, String, String, Option<String>)>(
        "SELECT id, display_name, username, email, company FROM convena.users WHERE email = $1 OR username = $1 LIMIT 1",
    )
    .bind(&identifier)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    let (target_user_id, target_name, _target_username, target_email, _target_company) = match user_lookup {
        Some(u) => u,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "message": format!("Pengguna dengan email atau username '{identifier}' belum terdaftar di Convena."),
                    "code": "USER_NOT_FOUND"
                })),
            );
        }
    };

    let member_id = Uuid::new_v4();
    let now = chrono::Utc::now();

    let insert_sql = r#"
        INSERT INTO convena.organization_members (
            id, organization_id, user_id, role_id, custom_title, status, joined_at, updated_at
        ) VALUES ($1, $2, $3, $4, $5, 'active', $6, $6)
        ON CONFLICT (organization_id, user_id) 
        DO UPDATE SET 
            role_id = EXCLUDED.role_id,
            custom_title = COALESCE(EXCLUDED.custom_title, convena.organization_members.custom_title),
            status = 'active',
            updated_at = EXCLUDED.updated_at
        RETURNING id, organization_id, user_id, role_id, custom_title, status, joined_at, updated_at
    "#;

    match db::query(insert_sql)
        .bind(member_id)
        .bind(org_id)
        .bind(target_user_id)
        .bind(payload.role_id)
        .bind(payload.custom_title)
        .bind(now)
        .execute(&state.pool)
        .await
    {
        Ok(_) => (
            StatusCode::CREATED,
            Json(json!({
                "message": format!("Anggota {target_name} ({target_email}) berhasil ditambahkan ke organisasi."),
                "success": true
            })),
        ),
        Err(e) => {
            tracing::error!("Failed to add member to organization: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal menambahkan anggota ke organisasi", "code": "DB_ERROR" })),
            )
        }
    }
}

/// PATCH /organizations/{id}/members/{member_id} - Update member's role or custom title
pub async fn update_member_handler(
    State(state): State<AppState>,
    Path((org_id, member_id)): Path<(Uuid, Uuid)>,
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

    let can_manage = match check_org_permission(&state.pool, current_user_id, org_id, "org:manage_members").await {
        Ok(allowed) => allowed,
        Err(_) => false,
    };

    if !can_manage {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Anda tidak memiliki izin mengubah data anggota organisasi", "code": "FORBIDDEN" })),
        );
    }

    // Verify role belongs to organization
    let role_exists: bool = db::query_scalar::<sqlx::Postgres, bool>(
        "SELECT EXISTS (SELECT 1 FROM convena.organization_roles WHERE id = $1 AND organization_id = $2)",
    )
    .bind(payload.role_id)
    .bind(org_id)
    .fetch_one(&state.pool)
    .await
    .unwrap_or(false);

    if !role_exists {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "message": "Role tidak valid untuk organisasi ini", "code": "INVALID_ROLE" })),
        );
    }

    let sql = r#"
        UPDATE convena.organization_members
        SET 
            role_id = $1,
            custom_title = COALESCE($2, custom_title),
            updated_at = NOW()
        WHERE id = $3 AND organization_id = $4
    "#;

    match db::query(sql)
        .bind(payload.role_id)
        .bind(payload.custom_title)
        .bind(member_id)
        .bind(org_id)
        .execute(&state.pool)
        .await
    {
        Ok(res) if res.rows_affected() > 0 => (
            StatusCode::OK,
            Json(json!({ "message": "Peran anggota berhasil diperbarui", "success": true })),
        ),
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": "Anggota tidak ditemukan", "code": "MEMBER_NOT_FOUND" })),
        ),
        Err(e) => {
            tracing::error!("Failed to update member role: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal memperbarui peran anggota", "code": "DB_ERROR" })),
            )
        }
    }
}

/// DELETE /organizations/{id}/members/{member_id} - Remove member from organization
pub async fn remove_member_handler(
    State(state): State<AppState>,
    Path((org_id, member_id)): Path<(Uuid, Uuid)>,
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

    let can_manage = match check_org_permission(&state.pool, current_user_id, org_id, "org:manage_members").await {
        Ok(allowed) => allowed,
        Err(_) => false,
    };

    if !can_manage {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "message": "Anda tidak memiliki izin mengeluarkan anggota dari organisasi", "code": "FORBIDDEN" })),
        );
    }

    // Check if target member is owner
    let member_role: Option<(String,)> = db::query_as::<sqlx::Postgres, (String,)>(
        r#"
            SELECT r.name 
            FROM convena.organization_members m
            INNER JOIN convena.organization_roles r ON m.role_id = r.id
            WHERE m.id = $1 AND m.organization_id = $2
        "#,
    )
    .bind(member_id)
    .bind(org_id)
    .fetch_optional(&state.pool)
    .await
    .unwrap_or(None);

    if let Some((role_name,)) = member_role {
        if role_name == "owner" {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "message": "Pemilik organisasi (Owner) tidak dapat dikeluarkan.", "code": "CANNOT_REMOVE_OWNER" })),
            );
        }
    }

    match db::query("DELETE FROM convena.organization_members WHERE id = $1 AND organization_id = $2")
        .bind(member_id)
        .bind(org_id)
        .execute(&state.pool)
        .await
    {
        Ok(res) if res.rows_affected() > 0 => (
            StatusCode::OK,
            Json(json!({ "message": "Anggota berhasil dikeluarkan dari organisasi", "success": true })),
        ),
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": "Anggota tidak ditemukan", "code": "MEMBER_NOT_FOUND" })),
        ),
        Err(e) => {
            tracing::error!("Failed to remove member: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal mengeluarkan anggota", "code": "DB_ERROR" })),
            )
        }
    }
}
