use sqlx::PgPool;
use uuid::Uuid;

use crate::db;
use crate::models::membership::{EventPermission, EventRole};

/// Fetch the role of a user within a specific event from `convena.event_members`.
pub async fn get_user_event_role(
    pool: &PgPool,
    user_id: Uuid,
    event_id: Uuid,
) -> Result<Option<EventRole>, sqlx::Error> {
    let sql = r#"
        SELECT role
        FROM convena.event_members
        WHERE event_id = $1 AND user_id = $2 AND status = 'active'
    "#;

    let row = db::query_as::<sqlx::Postgres, (String,)>(sql)
        .bind(event_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;

    Ok(row.and_then(|(r,)| EventRole::from_str(&r)))
}

/// Check if a user has a specific permission in an event.
pub async fn check_event_permission(
    pool: &PgPool,
    user_id: Uuid,
    event_id: Uuid,
    permission: EventPermission,
) -> Result<bool, sqlx::Error> {
    let role = get_user_event_role(pool, user_id, event_id).await?;
    match role {
        Some(r) => Ok(r.has_permission(permission)),
        None => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_permissions_matrix() {
        // Owner has everything
        assert!(EventRole::Owner.has_permission(EventPermission::EventDelete));
        assert!(EventRole::Owner.has_permission(EventPermission::EventEditSettings));
        assert!(EventRole::Owner.has_permission(EventPermission::TeamManageRoles));
        assert!(EventRole::Owner.has_permission(EventPermission::FinanceAuditSpj));

        // Admin has everything EXCEPT event delete
        assert!(!EventRole::Admin.has_permission(EventPermission::EventDelete));
        assert!(EventRole::Admin.has_permission(EventPermission::EventEditSettings));
        assert!(EventRole::Admin.has_permission(EventPermission::TeamInviteMember));
        assert!(EventRole::Admin.has_permission(EventPermission::FinanceAuditSpj));

        // Committee can scan tickets, submit SPJ, manage sponsors, but CANNOT audit SPJ or edit event settings
        assert!(EventRole::Committee.has_permission(EventPermission::TicketsScanCheckin));
        assert!(EventRole::Committee.has_permission(EventPermission::FinanceSubmitSpj));
        assert!(EventRole::Committee.has_permission(EventPermission::SponsorshipManage));
        assert!(!EventRole::Committee.has_permission(EventPermission::FinanceAuditSpj));
        assert!(!EventRole::Committee.has_permission(EventPermission::EventEditSettings));

        // Treasurer can view budget, submit SPJ, and AUDIT SPJ, but CANNOT scan tickets or manage sponsors
        assert!(EventRole::Treasurer.has_permission(EventPermission::FinanceAuditSpj));
        assert!(EventRole::Treasurer.has_permission(EventPermission::FinanceViewBudget));
        assert!(!EventRole::Treasurer.has_permission(EventPermission::TicketsScanCheckin));
        assert!(!EventRole::Treasurer.has_permission(EventPermission::SponsorshipManage));

        // Attendee can only view own ticket
        assert!(EventRole::Attendee.has_permission(EventPermission::AttendeeViewOwnTicket));
        assert!(!EventRole::Attendee.has_permission(EventPermission::FinanceViewBudget));
        assert!(!EventRole::Attendee.has_permission(EventPermission::TicketsScanCheckin));
        assert!(!EventRole::Attendee.has_permission(EventPermission::TeamViewMembers));
    }

    #[test]
    fn test_role_serialization() {
        assert_eq!(EventRole::from_str("owner"), Some(EventRole::Owner));
        assert_eq!(EventRole::from_str("ADMIN"), Some(EventRole::Admin));
        assert_eq!(EventRole::from_str("committee"), Some(EventRole::Committee));
        assert_eq!(EventRole::from_str("treasurer"), Some(EventRole::Treasurer));
        assert_eq!(EventRole::from_str("sponsor"), Some(EventRole::Sponsor));
        assert_eq!(EventRole::from_str("attendee"), Some(EventRole::Attendee));
        assert_eq!(EventRole::from_str("invalid_role"), None);
    }
}
