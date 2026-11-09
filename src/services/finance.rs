use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, patch, post},
    Json, Router,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    appstate::AppState,
    db,
    models::{
        event::TicketTier,
        finance::{
            BudgetCategoryPlan, CreateExpensePayload, EventBudgetSummary, ExpenseRecord,
            UpdateCategoryBudgetPayload, UpdateExpensePayload, VerifySpjPayload,
            DEFAULT_BUDGET_CATEGORIES,
        },
        membership::EventPermission,
    },
    services::{
        auth::extract_user_id_from_headers, events::parse_datetime, rbac::get_user_event_role,
    },
};

pub fn finance_routes() -> Router<AppState> {
    Router::new()
        .route("/summary/{event_id}", get(get_budget_summary_handler))
        .route(
            "/summary/{event_id}/category",
            patch(update_category_budget_handler),
        )
        .route("/expenses", post(create_expense_body_handler))
        .route(
            "/expenses/{event_id}",
            get(list_expenses_handler).post(create_expense_handler),
        )
        .route(
            "/expenses/{event_id}/{expense_id}",
            patch(update_expense_handler)
                .put(update_expense_handler)
                .delete(delete_expense_handler),
        )
        .route(
            "/expenses/{event_id}/{expense_id}/verify",
            patch(verify_spj_handler),
        )
}

fn is_seeded_dev_event(event_id: Uuid) -> bool {
    let s = event_id.to_string();
    matches!(
        s.as_str(),
        "11111111-1111-1111-1111-111111111101"
            | "11111111-1111-1111-1111-111111111102"
            | "11111111-1111-1111-1111-111111111103"
    )
}

async fn check_finance_access(
    pool: &PgPool,
    user_id: Uuid,
    event_id: Uuid,
    permission: EventPermission,
) -> Result<bool, sqlx::Error> {
    if let Some(role) = get_user_event_role(pool, user_id, event_id).await? {
        return Ok(role.has_permission(permission));
    }

    // Check if the user is the event creator
    let created_by: Option<Uuid> = db::query_scalar::<sqlx::Postgres, Uuid>(
        "SELECT created_by FROM convena.events WHERE id = $1",
    )
    .bind(event_id)
    .fetch_optional(pool)
    .await?;

    if created_by == Some(user_id) {
        return Ok(true);
    }

    // Allow authenticated developer accounts to test finance on seeded dev events
    if is_seeded_dev_event(event_id) {
        return Ok(true);
    }

    Ok(false)
}

/// Ensure all default RAB categories exist for the given `event_id`.
pub async fn ensure_default_categories(pool: &PgPool, event_id: Uuid) -> Result<(), sqlx::Error> {
    let insert_sql = r#"
        INSERT INTO convena.event_budget_categories (
            id, event_id, category, label, planned_amount, created_at, updated_at
        ) VALUES ($1, $2, $3, $4, 0, NOW(), NOW())
        ON CONFLICT (event_id, category) DO NOTHING
    "#;

    for (cat, label) in DEFAULT_BUDGET_CATEGORIES {
        db::query(insert_sql)
            .bind(Uuid::new_v4())
            .bind(event_id)
            .bind(*cat)
            .bind(*label)
            .execute(pool)
            .await?;
    }

    Ok(())
}

/// Build the full `EventBudgetSummary` for a specific `event_id`.
pub async fn build_budget_summary(
    pool: &PgPool,
    event_id: Uuid,
) -> Result<Option<EventBudgetSummary>, sqlx::Error> {
    // 1. Check event existence and fetch ticket_tiers to compute realized ticket income
    let event_row: Option<(sqlx::types::Json<Vec<TicketTier>>,)> = db::query_as(
        "SELECT ticket_tiers FROM convena.events WHERE id = $1",
    )
    .bind(event_id)
    .fetch_optional(pool)
    .await?;

    let (ticket_tiers,) = match event_row {
        Some(row) => row,
        None => return Ok(None),
    };

    let total_income_realized: f64 = ticket_tiers
        .0
        .iter()
        .map(|t| t.price * (t.sold.max(0) as f64))
        .sum();

    // 2. Ensure default RAB categories exist for this event
    ensure_default_categories(pool, event_id).await?;

    // 3. Query categories with dynamically aggregated spent_amount from non-rejected expenses
    let categories_sql = r#"
        SELECT
            c.id,
            c.event_id,
            c.category,
            c.label,
            c.planned_amount,
            COALESCE(SUM(CASE WHEN e.spj_status != 'rejected' THEN e.amount ELSE 0 END), 0)::float8 AS spent_amount
        FROM convena.event_budget_categories c
        LEFT JOIN convena.event_expenses e
            ON e.event_id = c.event_id AND e.category = c.category
        WHERE c.event_id = $1
        GROUP BY c.id, c.event_id, c.category, c.label, c.planned_amount
        ORDER BY
            CASE c.category
                WHEN 'venue' THEN 1
                WHEN 'catering' THEN 2
                WHEN 'equipment' THEN 3
                WHEN 'marketing' THEN 4
                WHEN 'speaker_honor' THEN 5
                WHEN 'logistics' THEN 6
                WHEN 'other' THEN 7
                ELSE 8
            END ASC
    "#;

    let categories = db::query_as::<sqlx::Postgres, BudgetCategoryPlan>(categories_sql)
        .bind(event_id)
        .fetch_all(pool)
        .await?;

    // 4. Compute totals and pending SPJ count
    let total_budget_planned: f64 = categories.iter().map(|c| c.planned_amount).sum();
    let total_expense_realized: f64 = categories.iter().map(|c| c.spent_amount).sum();
    let remaining_budget = total_budget_planned - total_expense_realized;

    let pending_spj_count: i64 = db::query_scalar::<sqlx::Postgres, i64>(
        "SELECT COUNT(*) FROM convena.event_expenses WHERE event_id = $1 AND spj_status = 'submitted'",
    )
    .bind(event_id)
    .fetch_one(pool)
    .await
    .unwrap_or(0);

    Ok(Some(EventBudgetSummary {
        event_id,
        total_budget_planned,
        total_income_realized,
        total_expense_realized,
        remaining_budget,
        pending_spj_count,
        categories,
    }))
}

/// GET `/finances/summary/{event_id}`
pub async fn get_budget_summary_handler(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
) -> impl IntoResponse {
    match build_budget_summary(&state.pool, event_id).await {
        Ok(Some(summary)) => (StatusCode::OK, Json(json!(summary))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "message": "Event tidak ditemukan",
                "code": "EVENT_NOT_FOUND"
            })),
        ),
        Err(e) => {
            tracing::error!("Failed to get budget summary for event {event_id}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "message": "Gagal memuat ringkasan anggaran event",
                    "code": "DB_ERROR"
                })),
            )
        }
    }
}

/// PATCH `/finances/summary/{event_id}/category`
pub async fn update_category_budget_handler(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<UpdateCategoryBudgetPayload>,
) -> impl IntoResponse {
    let current_user_id = match extract_user_id_from_headers(&headers, &state.config.jwt_secret) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "message": "Otorisasi diperlukan untuk mengubah pagu anggaran",
                    "code": "UNAUTHORIZED"
                })),
            );
        }
    };

    let allowed = match check_finance_access(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::FinanceAuditSpj,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("Permission check error on update_category_budget: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan otorisasi", "code": "AUTH_ERROR" })),
            );
        }
    };

    if !allowed {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "message": "Hanya Owner, Admin, atau Bendahara event yang dapat mengubah pagu RAB",
                "code": "FORBIDDEN"
            })),
        );
    }

    let category = payload.category.trim().to_lowercase();
    let planned_amount = payload.planned_amount.max(0.0);

    let default_label = DEFAULT_BUDGET_CATEGORIES
        .iter()
        .find(|(c, _)| *c == category)
        .map(|(_, l)| *l)
        .unwrap_or("Biaya Lain-lain & Tak Terduga");

    let upsert_sql = r#"
        INSERT INTO convena.event_budget_categories (
            id, event_id, category, label, planned_amount, created_at, updated_at
        ) VALUES ($1, $2, $3, $4, $5, NOW(), NOW())
        ON CONFLICT (event_id, category)
        DO UPDATE SET planned_amount = EXCLUDED.planned_amount, updated_at = NOW()
    "#;

    if let Err(e) = db::query(upsert_sql)
        .bind(Uuid::new_v4())
        .bind(event_id)
        .bind(&category)
        .bind(default_label)
        .bind(planned_amount)
        .execute(&state.pool)
        .await
    {
        tracing::error!("Failed to update category budget for event {event_id}: {e}");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "message": "Gagal memperbarui pagu anggaran kategori",
                "code": "DB_ERROR"
            })),
        );
    }

    match build_budget_summary(&state.pool, event_id).await {
        Ok(Some(summary)) => (StatusCode::OK, Json(json!(summary))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "message": "Event tidak ditemukan", "code": "EVENT_NOT_FOUND" })),
        ),
        Err(e) => {
            tracing::error!("Failed to rebuild budget summary: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Gagal memuat anggaran terbaru", "code": "DB_ERROR" })),
            )
        }
    }
}

/// GET `/finances/expenses/{event_id}`
pub async fn list_expenses_handler(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
) -> impl IntoResponse {
    let sql = r#"
        SELECT
            id, event_id, title, category, amount, receipt_url, receipt_date,
            submitted_by_id, submitted_by, division, spj_status,
            approved_by_id, approved_by, notes, created_at, updated_at
        FROM convena.event_expenses
        WHERE event_id = $1
        ORDER BY receipt_date DESC, created_at DESC
    "#;

    match db::query_as::<sqlx::Postgres, ExpenseRecord>(sql)
        .bind(event_id)
        .fetch_all(&state.pool)
        .await
    {
        Ok(expenses) => (StatusCode::OK, Json(json!(expenses))),
        Err(e) => {
            tracing::error!("Failed to list expenses for event {event_id}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "message": "Gagal memuat daftar pengeluaran event",
                    "code": "DB_ERROR"
                })),
            )
        }
    }
}

/// Internal helper to create an expense for a given `event_id`.
async fn create_expense_for_event(
    state: AppState,
    event_id: Uuid,
    headers: HeaderMap,
    payload: CreateExpensePayload,
) -> (StatusCode, Json<serde_json::Value>) {
    let current_user_id = match extract_user_id_from_headers(&headers, &state.config.jwt_secret) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(json!({
                    "message": "Otorisasi diperlukan untuk mencatat pengeluaran",
                    "code": "UNAUTHORIZED"
                })),
            );
        }
    };

    let allowed = match check_finance_access(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::FinanceSubmitSpj,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("Permission check error on create_expense: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan otorisasi", "code": "AUTH_ERROR" })),
            );
        }
    };

    if !allowed {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "message": "Anda tidak memiliki izin mencatat pengeluaran di event ini",
                "code": "FORBIDDEN"
            })),
        );
    }

    let title = payload.title.trim().to_string();
    if title.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "message": "Nama transaksi / pengeluaran wajib diisi",
                "code": "MISSING_TITLE"
            })),
        );
    }

    if payload.amount <= 0.0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "message": "Nominal pengeluaran harus lebih besar dari 0",
                "code": "INVALID_AMOUNT"
            })),
        );
    }

    let category = payload.category.trim().to_lowercase();
    let division = payload
        .division
        .map(|d| d.trim().to_string())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "Divisi Kepanitiaan".to_string());

    let receipt_date = payload
        .receipt_date
        .as_deref()
        .and_then(parse_datetime)
        .unwrap_or_else(chrono::Utc::now);

    // Resolve submitter display name from database if not explicitly provided
    let db_display_name: Option<String> = db::query_scalar::<sqlx::Postgres, String>(
        "SELECT display_name FROM convena.users WHERE id = $1",
    )
    .bind(current_user_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten();

    let submitted_by = payload
        .submitted_by
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or(db_display_name)
        .unwrap_or_else(|| "Anggota Panitia".to_string());

    let _ = ensure_default_categories(&state.pool, event_id).await;

    let expense_id = Uuid::new_v4();
    let now = chrono::Utc::now();

    let insert_sql = r#"
        INSERT INTO convena.event_expenses (
            id, event_id, title, category, amount, receipt_url, receipt_date,
            submitted_by_id, submitted_by, division, spj_status,
            notes, created_at, updated_at
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7,
            $8, $9, $10, 'submitted',
            $11, $12, $12
        )
        RETURNING
            id, event_id, title, category, amount, receipt_url, receipt_date,
            submitted_by_id, submitted_by, division, spj_status,
            approved_by_id, approved_by, notes, created_at, updated_at
    "#;

    match db::query_as::<sqlx::Postgres, ExpenseRecord>(insert_sql)
        .bind(expense_id)
        .bind(event_id)
        .bind(&title)
        .bind(&category)
        .bind(payload.amount)
        .bind(payload.receipt_url)
        .bind(receipt_date)
        .bind(current_user_id)
        .bind(&submitted_by)
        .bind(&division)
        .bind(payload.notes)
        .bind(now)
        .fetch_one(&state.pool)
        .await
    {
        Ok(record) => (StatusCode::CREATED, Json(json!(record))),
        Err(e) => {
            tracing::error!("Failed to insert expense for event {event_id}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "message": "Gagal menyimpan data pengeluaran",
                    "code": "DB_ERROR"
                })),
            )
        }
    }
}

/// POST `/finances/expenses/{event_id}`
pub async fn create_expense_handler(
    State(state): State<AppState>,
    Path(event_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<CreateExpensePayload>,
) -> impl IntoResponse {
    create_expense_for_event(state, event_id, headers, payload).await
}

/// POST `/finances/expenses` (with `eventId` in payload body)
pub async fn create_expense_body_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<CreateExpensePayload>,
) -> impl IntoResponse {
    let event_id = match payload.event_id {
        Some(id) => id,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "message": "eventId wajib disertakan untuk mencatat pengeluaran",
                    "code": "MISSING_EVENT_ID"
                })),
            );
        }
    };
    create_expense_for_event(state, event_id, headers, payload).await
}

/// PATCH / PUT `/finances/expenses/{event_id}/{expense_id}`
pub async fn update_expense_handler(
    State(state): State<AppState>,
    Path((event_id, expense_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<UpdateExpensePayload>,
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

    let allowed = match check_finance_access(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::FinanceSubmitSpj,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("Permission check error on update_expense: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan otorisasi", "code": "AUTH_ERROR" })),
            );
        }
    };

    if !allowed {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "message": "Anda tidak memiliki izin mengubah data pengeluaran di event ini",
                "code": "FORBIDDEN"
            })),
        );
    }

    if let Some(amount) = payload.amount {
        if amount <= 0.0 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "message": "Nominal pengeluaran harus lebih besar dari 0",
                    "code": "INVALID_AMOUNT"
                })),
            );
        }
    }

    let parsed_receipt_date = payload.receipt_date.as_deref().and_then(parse_datetime);

    let update_sql = r#"
        UPDATE convena.event_expenses
        SET
            title = COALESCE($1, title),
            category = COALESCE($2, category),
            amount = COALESCE($3, amount),
            receipt_url = COALESCE($4, receipt_url),
            receipt_date = COALESCE($5, receipt_date),
            division = COALESCE($6, division),
            notes = COALESCE($7, notes),
            updated_at = NOW()
        WHERE id = $8 AND event_id = $9
        RETURNING
            id, event_id, title, category, amount, receipt_url, receipt_date,
            submitted_by_id, submitted_by, division, spj_status,
            approved_by_id, approved_by, notes, created_at, updated_at
    "#;

    match db::query_as::<sqlx::Postgres, ExpenseRecord>(update_sql)
        .bind(payload.title.map(|t| t.trim().to_string()).filter(|t| !t.is_empty()))
        .bind(
            payload
                .category
                .map(|c| c.trim().to_lowercase())
                .filter(|c| !c.is_empty()),
        )
        .bind(payload.amount)
        .bind(payload.receipt_url)
        .bind(parsed_receipt_date)
        .bind(
            payload
                .division
                .map(|d| d.trim().to_string())
                .filter(|d| !d.is_empty()),
        )
        .bind(payload.notes)
        .bind(expense_id)
        .bind(event_id)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(Some(record)) => (StatusCode::OK, Json(json!(record))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "message": "Data pengeluaran tidak ditemukan pada event ini",
                "code": "EXPENSE_NOT_FOUND"
            })),
        ),
        Err(e) => {
            tracing::error!("Failed to update expense {expense_id} in event {event_id}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "message": "Gagal memperbarui data pengeluaran",
                    "code": "DB_ERROR"
                })),
            )
        }
    }
}

/// DELETE `/finances/expenses/{event_id}/{expense_id}`
pub async fn delete_expense_handler(
    State(state): State<AppState>,
    Path((event_id, expense_id)): Path<(Uuid, Uuid)>,
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

    let allowed = match check_finance_access(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::FinanceSubmitSpj,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("Permission check error on delete_expense: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan otorisasi", "code": "AUTH_ERROR" })),
            );
        }
    };

    if !allowed {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "message": "Anda tidak memiliki izin menghapus pengeluaran di event ini",
                "code": "FORBIDDEN"
            })),
        );
    }

    let delete_sql = "DELETE FROM convena.event_expenses WHERE id = $1 AND event_id = $2";
    match db::query(delete_sql)
        .bind(expense_id)
        .bind(event_id)
        .execute(&state.pool)
        .await
    {
        Ok(res) if res.rows_affected() > 0 => (
            StatusCode::OK,
            Json(json!({
                "message": "Data pengeluaran berhasil dihapus",
                "id": expense_id
            })),
        ),
        Ok(_) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "message": "Data pengeluaran tidak ditemukan pada event ini",
                "code": "EXPENSE_NOT_FOUND"
            })),
        ),
        Err(e) => {
            tracing::error!("Failed to delete expense {expense_id} in event {event_id}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "message": "Gagal menghapus data pengeluaran",
                    "code": "DB_ERROR"
                })),
            )
        }
    }
}

/// PATCH `/finances/expenses/{event_id}/{expense_id}/verify`
pub async fn verify_spj_handler(
    State(state): State<AppState>,
    Path((event_id, expense_id)): Path<(Uuid, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<VerifySpjPayload>,
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

    let allowed = match check_finance_access(
        &state.pool,
        current_user_id,
        event_id,
        EventPermission::FinanceAuditSpj,
    )
    .await
    {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("Permission check error on verify_spj: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "message": "Kesalahan otorisasi", "code": "AUTH_ERROR" })),
            );
        }
    };

    if !allowed {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "message": "Hanya Owner, Admin, atau Bendahara event yang dapat memverifikasi kuitansi SPJ",
                "code": "FORBIDDEN"
            })),
        );
    }

    let approver_name: String = db::query_scalar::<sqlx::Postgres, String>(
        "SELECT display_name FROM convena.users WHERE id = $1",
    )
    .bind(current_user_id)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
    .unwrap_or_else(|| "Bendahara Event".to_string());

    let new_status = if payload.approved {
        "verified"
    } else {
        "rejected"
    };

    let sql = r#"
        UPDATE convena.event_expenses
        SET
            spj_status = $1,
            approved_by_id = $2,
            approved_by = $3,
            notes = COALESCE($4, notes),
            updated_at = NOW()
        WHERE id = $5 AND event_id = $6
        RETURNING
            id, event_id, title, category, amount, receipt_url, receipt_date,
            submitted_by_id, submitted_by, division, spj_status,
            approved_by_id, approved_by, notes, created_at, updated_at
    "#;

    match db::query_as::<sqlx::Postgres, ExpenseRecord>(sql)
        .bind(new_status)
        .bind(current_user_id)
        .bind(&approver_name)
        .bind(payload.notes)
        .bind(expense_id)
        .bind(event_id)
        .fetch_optional(&state.pool)
        .await
    {
        Ok(Some(record)) => (StatusCode::OK, Json(json!(record))),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "message": "Data pengeluaran tidak ditemukan pada event ini",
                "code": "EXPENSE_NOT_FOUND"
            })),
        ),
        Err(e) => {
            tracing::error!("Failed to verify SPJ {expense_id} in event {event_id}: {e}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "message": "Gagal memverifikasi status SPJ",
                    "code": "DB_ERROR"
                })),
            )
        }
    }
}
