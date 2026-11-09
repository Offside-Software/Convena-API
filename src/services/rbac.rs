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
    // 1. Direct event committee membership check
    let role = get_user_event_role(pool, user_id, event_id).await?;
    if let Some(r) = role {
        if r.has_permission(permission) {
            return Ok(true);
        }
    }

    // 2. Organization inheritance: if the event belongs to an organization,
    // organization owner/admin or members with corresponding org permission also have access.
    let org_query = r#"
        SELECT organization_id
        FROM convena.events
        WHERE id = $1
    "#;
    let org_row = db::query_as::<sqlx::Postgres, (Option<Uuid>,)>(org_query)
        .bind(event_id)
        .fetch_optional(pool)
        .await?;

    if let Some((Some(org_id),)) = org_row {
        let (role_name, _, perms) = match get_user_org_role_and_permissions(pool, user_id, org_id).await? {
            Some(res) => res,
            None => return Ok(false),
        };

        if role_name == "owner" {
            return Ok(true);
        }

        // Map EventPermission to permission string key
        let perm_key = match permission {
            EventPermission::EventEditSettings => "event:edit",
            EventPermission::EventDelete => "event:delete",
            EventPermission::TeamInviteMember | EventPermission::TeamManageRoles | EventPermission::TeamViewMembers => "event:manage_team",
            EventPermission::TicketsManageTiers | EventPermission::TicketsScanCheckin => "event:edit",
            EventPermission::FinanceViewBudget => "finance:view",
            EventPermission::FinanceSubmitSpj => "finance:submit_expense",
            EventPermission::FinanceAuditSpj => "finance:audit_spj",
            EventPermission::SponsorshipManage | EventPermission::SponsorshipDeliverables => "sponsor:manage",
            EventPermission::CertificateIssue => "cert:issue",
            EventPermission::AttendeeViewOwnTicket => return Ok(true),
        };

        if perms.contains(&perm_key.to_string()) {
            return Ok(true);
        }
    }

    Ok(false)
}

/// Fetch the role name, display name, and active permissions array for a user in an organization.
pub async fn get_user_org_role_and_permissions(
    pool: &PgPool,
    user_id: Uuid,
    org_id: Uuid,
) -> Result<Option<(String, String, Vec<String>)>, sqlx::Error> {
    let sql = r#"
        SELECT r.name, r.display_name, r.permissions
        FROM convena.organization_members m
        INNER JOIN convena.organization_roles r ON m.role_id = r.id
        WHERE m.organization_id = $1 AND m.user_id = $2 AND m.status = 'active'
    "#;

    let row = db::query_as::<sqlx::Postgres, (String, String, sqlx::types::Json<Vec<String>>)>(sql)
        .bind(org_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|(name, display_name, perms)| (name, display_name, perms.0)))
}

/// Check if a user has a specific permission in an organization.
/// The 'owner' role always has all permissions.
pub async fn check_org_permission(
    pool: &PgPool,
    user_id: Uuid,
    org_id: Uuid,
    permission: &str,
) -> Result<bool, sqlx::Error> {
    let user_role_perms = get_user_org_role_and_permissions(pool, user_id, org_id).await?;
    match user_role_perms {
        Some((role_name, _, perms)) => {
            if role_name == "owner" {
                return Ok(true);
            }
            Ok(perms.iter().any(|p| p == permission))
        }
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

    #[test]
    fn test_default_system_roles_integrity() {
        use crate::models::organization::{DEFAULT_SYSTEM_ROLES, PERMISSIONS_CATALOG};

        let valid_perm_keys: std::collections::HashSet<&str> =
            PERMISSIONS_CATALOG.iter().map(|(k, _, _, _)| *k).collect();

        for (name, _, _, perms) in DEFAULT_SYSTEM_ROLES {
            assert!(!name.is_empty());
            for perm in *perms {
                assert!(
                    valid_perm_keys.contains(perm),
                    "Permission {perm} in role {name} does not exist in PERMISSIONS_CATALOG"
                );
            }
        }

        // Owner must have all permissions
        let owner_perms = DEFAULT_SYSTEM_ROLES
            .iter()
            .find(|(n, _, _, _)| *n == "owner")
            .unwrap()
            .3;
        assert_eq!(owner_perms.len(), PERMISSIONS_CATALOG.len());
    }
}
