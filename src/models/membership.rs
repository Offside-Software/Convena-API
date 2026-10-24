use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventRole {
    Owner,
    Admin,
    Committee,
    Treasurer,
    Sponsor,
    Attendee,
}

impl EventRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            EventRole::Owner => "owner",
            EventRole::Admin => "admin",
            EventRole::Committee => "committee",
            EventRole::Treasurer => "treasurer",
            EventRole::Sponsor => "sponsor",
            EventRole::Attendee => "attendee",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "owner" => Some(EventRole::Owner),
            "admin" => Some(EventRole::Admin),
            "committee" => Some(EventRole::Committee),
            "treasurer" => Some(EventRole::Treasurer),
            "sponsor" => Some(EventRole::Sponsor),
            "attendee" => Some(EventRole::Attendee),
            _ => None,
        }
    }

    pub fn has_permission(&self, permission: EventPermission) -> bool {
        match self {
            EventRole::Owner => true, // Owner has all permissions
            EventRole::Admin => match permission {
                EventPermission::EventDelete => false, // Only owner can delete the event
                _ => true,
            },
            EventRole::Committee => matches!(
                permission,
                EventPermission::TeamViewMembers
                    | EventPermission::TicketsScanCheckin
                    | EventPermission::FinanceViewBudget
                    | EventPermission::FinanceSubmitSpj
                    | EventPermission::SponsorshipManage
                    | EventPermission::SponsorshipDeliverables
                    | EventPermission::CertificateIssue
                    | EventPermission::AttendeeViewOwnTicket
            ),
            EventRole::Treasurer => matches!(
                permission,
                EventPermission::TeamViewMembers
                    | EventPermission::FinanceViewBudget
                    | EventPermission::FinanceSubmitSpj
                    | EventPermission::FinanceAuditSpj
                    | EventPermission::AttendeeViewOwnTicket
            ),
            EventRole::Sponsor => matches!(
                permission,
                EventPermission::SponsorshipDeliverables | EventPermission::AttendeeViewOwnTicket
            ),
            EventRole::Attendee => matches!(permission, EventPermission::AttendeeViewOwnTicket),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventPermission {
    EventEditSettings,
    EventDelete,
    TeamInviteMember,
    TeamManageRoles,
    TeamViewMembers,
    TicketsManageTiers,
    TicketsScanCheckin,
    FinanceViewBudget,
    FinanceSubmitSpj,
    FinanceAuditSpj,
    SponsorshipManage,
    SponsorshipDeliverables,
    CertificateIssue,
    AttendeeViewOwnTicket,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct EventMember {
    pub id: Uuid,
    pub event_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
    pub custom_title: Option<String>,
    pub status: String,
    pub joined_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct EventMemberDetail {
    pub id: Uuid,
    pub event_id: Uuid,
    pub user_id: Uuid,
    pub display_name: String,
    pub username: String,
    pub email: String,
    pub role: String,
    pub custom_title: Option<String>,
    pub status: String,
    pub joined_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct EventInvitation {
    pub id: Uuid,
    pub event_id: Uuid,
    pub invited_by: Uuid,
    pub email: String,
    pub role: String,
    pub custom_title: Option<String>,
    pub token: String,
    pub status: String,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteMemberPayload {
    pub email: String,
    pub role: String,
    pub custom_title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMemberRolePayload {
    pub role: String,
    pub custom_title: Option<String>,
}
