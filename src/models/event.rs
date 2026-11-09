use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TicketTier {
    pub id: String,
    pub name: String,
    pub price: f64,
    pub quota: i32,
    #[serde(default)]
    pub sold: i32,
    #[serde(default)]
    pub benefits: Vec<String>,
    #[serde(default = "default_true")]
    pub is_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub id: Uuid,
    pub created_by: Uuid,
    pub organization_id: Option<Uuid>,
    pub title: String,
    pub slug: String,
    pub description: Option<String>,
    pub short_description: Option<String>,
    pub banner_url: Option<String>,
    pub category: String,
    pub status: String,
    pub visibility: String,
    pub start_date: DateTime<Utc>,
    pub end_date: DateTime<Utc>,
    pub location_type: String,
    pub venue_name: Option<String>,
    pub address: Option<String>,
    pub city: Option<String>,
    pub online_url: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub attendance_qr_token: Option<String>,
    pub organization_name: Option<String>,
    pub company: Option<String>,
    #[serde(default)]
    pub ticket_tiers: sqlx::types::Json<Vec<TicketTier>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct EventWithRole {
    pub id: Uuid,
    pub created_by: Uuid,
    pub organization_id: Option<Uuid>,
    pub title: String,
    pub slug: String,
    pub description: Option<String>,
    pub short_description: Option<String>,
    pub banner_url: Option<String>,
    pub category: String,
    pub status: String,
    pub visibility: String,
    pub start_date: DateTime<Utc>,
    pub end_date: DateTime<Utc>,
    pub location_type: String,
    pub venue_name: Option<String>,
    pub address: Option<String>,
    pub city: Option<String>,
    pub online_url: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub attendance_qr_token: Option<String>,
    pub organization_name: Option<String>,
    pub company: Option<String>,
    #[serde(default)]
    pub ticket_tiers: sqlx::types::Json<Vec<TicketTier>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
    // Member-specific fields for the requesting user
    pub my_role: String,
    pub my_custom_title: Option<String>,
    pub my_division: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateEventPayload {
    pub title: String,
    pub slug: Option<String>,
    pub organization_id: Option<Uuid>,
    pub description: Option<String>,
    pub short_description: Option<String>,
    pub banner_url: Option<String>,
    pub category: String,
    pub status: Option<String>,
    pub visibility: Option<String>,
    pub start_date: String,
    pub end_date: String,
    pub location_type: Option<String>,
    pub venue_name: Option<String>,
    pub address: Option<String>,
    pub city: Option<String>,
    pub online_url: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub organization_name: Option<String>,
    pub company: Option<String>,
    pub ticket_tiers: Option<Vec<TicketTier>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateEventPayload {
    pub title: Option<String>,
    pub organization_id: Option<Uuid>,
    pub description: Option<String>,
    pub short_description: Option<String>,
    pub banner_url: Option<String>,
    pub category: Option<String>,
    pub status: Option<String>,
    pub visibility: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub location_type: Option<String>,
    pub venue_name: Option<String>,
    pub address: Option<String>,
    pub city: Option<String>,
    pub online_url: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub organization_name: Option<String>,
    pub company: Option<String>,
    pub ticket_tiers: Option<Vec<TicketTier>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct EventTask {
    pub id: Uuid,
    pub event_id: Uuid,
    pub title: String,
    pub description: String,
    pub assignee_id: Option<Uuid>,
    pub assignee_name: String,
    pub division: String,
    pub due_date: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateEventTaskPayload {
    pub title: String,
    pub description: Option<String>,
    pub assignee_id: Option<Uuid>,
    pub assignee_name: String,
    pub division: String,
    pub due_date: String,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateEventTaskPayload {
    pub title: Option<String>,
    pub description: Option<String>,
    pub assignee_id: Option<Uuid>,
    pub assignee_name: Option<String>,
    pub division: Option<String>,
    pub due_date: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct EventRegistration {
    pub id: Uuid,
    pub event_id: Uuid,
    pub user_id: Uuid,
    pub tier_id: String,
    pub tier_name: String,
    pub price: f64,
    pub payment_method: String,
    pub payment_status: String,
    pub status: String,
    pub attendee_name: String,
    pub attendee_email: String,
    pub attendee_phone: Option<String>,
    pub attendee_institution: Option<String>,
    pub ticket_code: String,
    pub checked_in_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterEventPayload {
    pub tier_id: String,
    pub payment_method: Option<String>,
    pub attendee_name: Option<String>,
    pub attendee_email: Option<String>,
    pub attendee_phone: Option<String>,
    pub attendee_institution: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttendEventQrPayload {
    pub qr_code: String,
    pub event_id: Option<Uuid>,
}
