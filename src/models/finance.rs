use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

pub const DEFAULT_BUDGET_CATEGORIES: &[(&str, &str)] = &[
    ("venue", "Sewa Gedung & Aula"),
    ("catering", "Konsumsi Peserta & Panitia"),
    ("equipment", "Sound, Lighting & Stage"),
    ("marketing", "Publikasi & Cetak Spanduk"),
    ("speaker_honor", "Honorarium Pembicara & Juri"),
    ("logistics", "Merchandise & Swag Kit"),
    ("other", "Biaya Lain-lain & Tak Terduga"),
];

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ExpenseRecord {
    pub id: Uuid,
    pub event_id: Uuid,
    pub title: String,
    pub category: String,
    pub amount: f64,
    pub receipt_url: Option<String>,
    pub receipt_date: DateTime<Utc>,
    pub submitted_by_id: Option<Uuid>,
    pub submitted_by: String,
    pub division: String,
    pub spj_status: String,
    pub approved_by_id: Option<Uuid>,
    pub approved_by: Option<String>,
    pub notes: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct BudgetCategoryPlan {
    pub id: Uuid,
    pub event_id: Uuid,
    pub category: String,
    pub label: String,
    pub planned_amount: f64,
    pub spent_amount: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventBudgetSummary {
    pub event_id: Uuid,
    pub total_budget_planned: f64,
    pub total_income_realized: f64,
    pub total_expense_realized: f64,
    pub remaining_budget: f64,
    pub pending_spj_count: i64,
    pub categories: Vec<BudgetCategoryPlan>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateExpensePayload {
    pub event_id: Option<Uuid>,
    pub title: String,
    pub category: String,
    pub amount: f64,
    pub receipt_url: Option<String>,
    pub receipt_date: Option<String>,
    pub division: Option<String>,
    pub submitted_by: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateExpensePayload {
    pub title: Option<String>,
    pub category: Option<String>,
    pub amount: Option<f64>,
    pub receipt_url: Option<String>,
    pub receipt_date: Option<String>,
    pub division: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifySpjPayload {
    pub approved: bool,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCategoryBudgetPayload {
    pub category: String,
    pub planned_amount: f64,
}
