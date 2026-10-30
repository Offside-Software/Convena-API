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
    pub organization_name: Option<String>,
    pub company: Option<String>,
    #[serde(default)]
    pub ticket_tiers: sqlx::types::Json<Vec<TicketTier>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
    // Member-specific fields for the requesting user
    pub my_role: String,
    pub my_custom_title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateEventPayload {
    pub title: String,
    pub slug: Option<String>,
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
    pub organization_name: Option<String>,
    pub company: Option<String>,
    pub ticket_tiers: Option<Vec<TicketTier>>,
}
