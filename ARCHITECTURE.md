# Convena Architecture & RBAC Design Document

## 1. Architectural Philosophy: Global Identity vs. Contextual (Event-Scoped) RBAC

In Convena, an account represents a **Global User Identity**, not a static system-wide operational role. 

Operational roles (such as `admin`, `committee`, `treasurer`, `sponsor`, `attendee`) **do not exist at the account level**. Instead, all roles are **Contextual and Scoped to an Event**.

### Why Account-Level Roles Are Anti-Patterns in Multi-Event Platforms:
- If a user chooses "Attendee" during registration, they would be barred from creating or managing an event later without admin intervention.
- In reality, university students and organizers switch hats across events:
  - **Project A (e.g. Arkavidia Festival)**:
    - **Arya**: `admin` / `owner` (Ketua Pelaksana)
    - **Michelle**: `committee` (Sie Acara)
    - **Tesa**: `attendee` (Ticket holder)
  - **Project B (e.g. National Hackathon)**:
    - **Tesa**: `admin` / `owner` (Project Lead)
    - **Arya**: `committee` (Technical Staff)
    - **Michelle**: `attendee` (Contestant)

### Global vs. Scoped Separation:
1. **Global Scope (`convena.users`)**:
   - Manages identity, authentication, profile (Name, Username, Email, Password Hash, University, Avatar).
   - Global role defaults to `'user'` (only platform operators hold `'system_admin'`).
2. **Event Scope (`convena.event_members`)**:
   - Manages role, permissions, and responsibilities for that specific event.
   - Every registered user can create an event. The creator is automatically granted the `owner` role for that event.
   - Event owners and admins can invite collaborators and assign event-scoped roles.

---

## 2. Entity-Relationship (ER) Model

```mermaid
erDiagram
    USERS ||--o{ EVENT_MEMBERS : "has membership in"
    EVENTS ||--o{ EVENT_MEMBERS : "has members"
    USERS ||--o{ EVENTS : "creates (as owner)"
    EVENTS ||--o{ EVENT_INVITATIONS : "issues"
    USERS ||--o{ EVENT_INVITATIONS : "invited by"
    EVENTS ||--o{ TICKET_REGISTRATIONS : "has attendees"
    USERS ||--o{ TICKET_REGISTRATIONS : "purchases / holds"

    USERS {
        uuid id PK
        string display_name
        string username UK
        string email UK
        string password_hash
        string global_role "user | system_admin"
        string university
        timestamptz created_at
        timestamptz updated_at
    }

    EVENTS {
        uuid id PK
        uuid created_by FK
        string title
        string slug UK
        string description
        string short_description
        string banner_url
        string category
        string status "draft | published | ongoing | completed | cancelled"
        string visibility "public | private | unlisted"
        timestamptz start_date
        timestamptz end_date
        string location_type "offline | online | hybrid"
        string venue_name
        string organization_name
        timestamptz created_at
    }

    EVENT_MEMBERS {
        uuid id PK
        uuid event_id FK
        uuid user_id FK
        string role "owner | admin | committee | treasurer | sponsor | attendee"
        string custom_title "e.g. Ketua Panitia, Sie Acara, Bendahara 1"
        string status "active | suspended"
        timestamptz joined_at
    }

    EVENT_INVITATIONS {
        uuid id PK
        uuid event_id FK
        uuid invited_by FK
        string email
        string role "admin | committee | treasurer | sponsor"
        string custom_title
        string token UK
        string status "pending | accepted | declined | expired"
        timestamptz expires_at
        timestamptz created_at
    }

    TICKET_REGISTRATIONS {
        uuid id PK
        uuid event_id FK
        uuid user_id FK
        uuid ticket_tier_id FK
        string qr_code UK
        string status "confirmed | checked_in | cancelled"
        timestamptz checked_in_at
        timestamptz created_at
    }
```

---

## 3. Event Role Hierarchy & Granular Permission Matrix

The platform uses a role-based permission evaluation engine. Operations in the backend check the user's role against the required `EventPermission`.

```
Owner
  └── Admin
        ├── Committee
        ├── Treasurer (Finance Focus)
        └── Sponsor Rep (Read-Only Deliverables)
```

| Permission Scope | Owner | Admin | Committee | Treasurer | Sponsor Rep | Attendee |
| :--- | :---: | :---: | :---: | :---: | :---: | :---: |
| `event:edit_settings` | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ |
| `event:delete` | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| `team:invite_member` | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ |
| `team:manage_roles` | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ |
| `team:view_members` | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| `tickets:manage_tiers` | ✅ | ✅ | ❌ | ❌ | ❌ | ❌ |
| `tickets:scan_checkin` | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ |
| `finance:view_budget` | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| `finance:submit_spj` | ✅ | ✅ | ✅ | ✅ | ❌ | ❌ |
| `finance:audit_spj` | ✅ | ✅ | ❌ | ✅ | ❌ | ❌ |
| `sponsorship:manage` | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ |
| `sponsorship:deliverables` | ✅ | ✅ | ✅ | ❌ | ✅ | ❌ |
| `certificate:issue` | ✅ | ✅ | ✅ | ❌ | ❌ | ❌ |
| `attendee:view_own_ticket` | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |

---

## 4. Lifecycle Flows

### 4.1 Account Registration
1. User provides `display_name`, `username`, `email`, and `password`.
2. Backend validates uniqueness, hashes password with Argon2 (id-argon2id), and inserts into `convena.users` with `global_role = 'user'`.
3. User is returned a JWT Bearer token containing their `user_id` and `email`.

### 4.2 Event Creation & Auto-Ownership
```mermaid
sequenceDiagram
    autonumber
    actor Organizer as Event Creator
    participant Web as Convena-Web
    participant API as Convena-API
    participant DB as PostgreSQL (Supabase)

    Organizer->>Web: Fill "Buat Event Baru" form
    Web->>API: POST /api/events (Bearer Token)
    API->>DB: INSERT INTO convena.events (...) RETURNING id
    API->>DB: INSERT INTO convena.event_members (event_id, user_id, role, custom_title) VALUES (event_id, user_id, 'owner', 'Event Creator')
    API-->>Web: 201 Created (Event Data + Member Role: owner)
    Web-->>Organizer: Redirect to Event Dashboard (Full Admin Privileges)
```

### 4.3 Inviting Team Members
```mermaid
sequenceDiagram
    autonumber
    actor Admin as Event Admin
    actor Invitee as Collaborator
    participant Web as Convena-Web
    participant API as Convena-API
    participant DB as PostgreSQL (Supabase)

    Admin->>Web: Select "Undang Anggota Panitia" (Email, Role, Title)
    Web->>API: POST /api/events/{id}/members/invite
    API->>DB: Verify Admin has 'team:invite_member' permission
    alt Invitee already exists in Convena
        API->>DB: Direct INSERT into convena.event_members
        API-->>Web: 200 OK (Member added directly)
    else Invitee does not have account
        API->>DB: INSERT INTO convena.event_invitations (token, expires_at)
        API-->>Web: 201 Created (Invitation link generated)
    end
```

---

## 5. Security & Authorization Architecture

### Backend (Rust / Axum):
1. **JWT Authentication Layer**:
   - Validates Bearer token on incoming HTTP requests.
   - Extracts `user_id` and attaches `CurrentUser` to request extensions.
2. **Event Authorization Guard**:
   - Reads `event_id` from URL path parameter (e.g. `/api/events/:event_id/...`).
   - Queries `convena.event_members` for `(event_id, user_id)`.
   - Compares the member's `role` against required permission matrix.
   - If unauthorized, rejects immediately with `403 Forbidden` (`INSUFFICIENT_EVENT_PERMISSIONS`).

### Frontend (React / Tailwind):
1. **Auth Context**: Stores authenticated user identity.
2. **Hook `useEventAuth(eventId)`**:
   - Queries current user's membership for the event.
   - Exposes boolean permission helpers: `canEditSettings`, `canAuditSpj`, `canScanTickets`, `isOwner`, `isAdmin`, `isCommittee`, `isTreasurer`.
3. **UI Permission Gates**:
   - Sensitive action buttons (e.g. "Verifikasi Kuitansi", "Edit Informasi Event", "Undang Tim") conditionally render based on `useEventAuth`.
