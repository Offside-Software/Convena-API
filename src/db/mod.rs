use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;
use std::str::FromStr;
use std::time::Duration;
use uuid::Uuid;

use crate::models::finance::DEFAULT_BUDGET_CATEGORIES;
use crate::models::organization::DEFAULT_SYSTEM_ROLES;
use crate::services::auth::hash_password;

pub fn get_connect_options(database_url: &str) -> Result<PgConnectOptions, sqlx::Error> {
    let options = PgConnectOptions::from_str(database_url)?
        .statement_cache_capacity(0); // Essential for Supabase / PgBouncer / Supavisor pooler
    Ok(options)
}

/// Help to construct queries with `.persistent(false)` for PgBouncer / Supavisor Compatibility.
/// Prevents named prepared statement collisions (`sqlx_s... already exists`) in transaction pooling mode.
pub fn query<'q>(
    sql: &'q str,
) -> sqlx::query::Query<'q, sqlx::Postgres, <sqlx::Postgres as sqlx::Database>::Arguments<'q>> {
    sqlx::query(sql).persistent(false)
}

/// Helper to construct query_as with `.persistent(false)` for PgBouncer / Supavisor compatibility.
pub fn query_as<'q, DB, O>(
    sql: &'q str,
) -> sqlx::query::QueryAs<'q, DB, O, <DB as sqlx::Database>::Arguments<'q>>
where
    DB: sqlx::Database + sqlx::database::HasStatementCache,
    O: for<'r> sqlx::FromRow<'r, DB::Row>,
{
    sqlx::query_as(sql).persistent(false)
}

/// Helper to construct query_scalar with `.persistent(false)` for PgBouncer / Supavisor compatibility.
pub fn query_scalar<'q, DB, O>(
    sql: &'q str,
) -> sqlx::query::QueryScalar<'q, DB, O, <DB as sqlx::Database>::Arguments<'q>>
where
    DB: sqlx::Database + sqlx::database::HasStatementCache,
    (O,): for<'r> sqlx::FromRow<'r, DB::Row>,
{
    sqlx::query_scalar(sql).persistent(false)
}

pub fn get_pool_options() -> PgPoolOptions {
    PgPoolOptions::new()
        // Supabase Session Pooler has a global per-project session limit (15 by default)
        // Keep our API pool conservative so it never hits EMAXCONNSESSION.
        .max_connections(3)
        .min_connections(0)
        .acquire_timeout(Duration::from_secs(10))
        .idle_timeout(Duration::from_secs(10))
        .max_lifetime(Duration::from_secs(5 * 60))
}

pub async fn create_pool(database_url: &str) -> Result<PgPool, sqlx::Error> {
    let options = get_connect_options(database_url)?;

    let pool = get_pool_options().connect_with(options).await?;

    init_database(&pool).await?;
    Ok(pool)
}

pub async fn init_database(pool: &PgPool) -> Result<(), sqlx::Error> {
    // Ensure Convena Schema & Tables
    let init_sql = r#"
        CREATE SCHEMA IF NOT EXISTS convena;

        CREATE TABLE IF NOT EXISTS convena.users (
            id UUID PRIMARY KEY,
            display_name VARCHAR(255) NOT NULL,
            username VARCHAR(100) UNIQUE NOT NULL,
            email VARCHAR(255) UNIQUE NOT NULL,
            password_hash TEXT NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            role VARCHAR(50) NOT NULL DEFAULT 'user',
            company VARCHAR(255)
        );

        CREATE INDEX IF NOT EXISTS idx_users_username ON convena.users (username);
        CREATE INDEX IF NOT EXISTS idx_users_email ON convena.users (email);

        CREATE TABLE IF NOT EXISTS convena.events (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            created_by UUID NOT NULL REFERENCES convena.users(id) ON DELETE CASCADE,
            title VARCHAR(255) NOT NULL,
            slug VARCHAR(255) UNIQUE NOT NULL,
            description TEXT,
            short_description VARCHAR(500),
            banner_url TEXT,
            category VARCHAR(50) NOT NULL,
            status VARCHAR(50) NOT NULL DEFAULT 'draft',
            visibility VARCHAR(50) NOT NULL DEFAULT 'public',
            start_date TIMESTAMPTZ NOT NULL,
            end_date TIMESTAMPTZ NOT NULL,
            location_type VARCHAR(50) NOT NULL DEFAULT 'offline',
            venue_name VARCHAR(255),
            address TEXT,
            city VARCHAR(100),
            online_url TEXT,
            organization_name VARCHAR(255),
            company VARCHAR(255),
            ticket_tiers JSONB NOT NULL DEFAULT '[]'::jsonb,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );

        DO $$
        BEGIN
            IF EXISTS (
                SELECT 1 FROM information_schema.columns 
                WHERE table_schema = 'convena' AND table_name = 'events' AND column_name = 'university'
            ) AND NOT EXISTS (
                SELECT 1 FROM information_schema.columns 
                WHERE table_schema = 'convena' AND table_name = 'events' AND column_name = 'company'
            ) THEN
                ALTER TABLE convena.events RENAME COLUMN university TO company;
            ELSE
                ALTER TABLE convena.events ADD COLUMN IF NOT EXISTS company VARCHAR(255);
            END IF;

            ALTER TABLE convena.events ADD COLUMN IF NOT EXISTS ticket_tiers JSONB NOT NULL DEFAULT '[]'::jsonb;
        END $$;

        CREATE INDEX IF NOT EXISTS idx_events_slug ON convena.events (slug);
        CREATE INDEX IF NOT EXISTS idx_events_created_by ON convena.events (created_by);
        CREATE INDEX IF NOT EXISTS idx_events_status ON convena.events (status);

        CREATE TABLE IF NOT EXISTS convena.event_members (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            event_id UUID NOT NULL REFERENCES convena.events(id) ON DELETE CASCADE,
            user_id UUID NOT NULL REFERENCES convena.users(id) ON DELETE CASCADE,
            role VARCHAR(50) NOT NULL,
            custom_title VARCHAR(100),
            status VARCHAR(50) NOT NULL DEFAULT 'active',
            joined_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            UNIQUE (event_id, user_id)
        );

        CREATE INDEX IF NOT EXISTS idx_event_members_user ON convena.event_members (user_id);
        CREATE INDEX IF NOT EXISTS idx_event_members_event ON convena.event_members (event_id);
        CREATE INDEX IF NOT EXISTS idx_event_members_role ON convena.event_members (role);

        CREATE TABLE IF NOT EXISTS convena.event_invitations (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            event_id UUID NOT NULL REFERENCES convena.events(id) ON DELETE CASCADE,
            invited_by UUID NOT NULL REFERENCES convena.users(id) ON DELETE CASCADE,
            email VARCHAR(255) NOT NULL,
            role VARCHAR(50) NOT NULL DEFAULT 'committee',
            custom_title VARCHAR(100),
            token VARCHAR(255) UNIQUE NOT NULL,
            status VARCHAR(50) NOT NULL DEFAULT 'pending',
            expires_at TIMESTAMPTZ NOT NULL,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );

        CREATE INDEX IF NOT EXISTS idx_event_invitations_token ON convena.event_invitations (token);
        CREATE INDEX IF NOT EXISTS idx_event_invitations_email ON convena.event_invitations (email);

        CREATE TABLE IF NOT EXISTS convena.event_budget_categories (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            event_id UUID NOT NULL REFERENCES convena.events(id) ON DELETE CASCADE,
            category VARCHAR(50) NOT NULL,
            label VARCHAR(255) NOT NULL,
            planned_amount DOUBLE PRECISION NOT NULL DEFAULT 0,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            UNIQUE (event_id, category)
        );

        CREATE INDEX IF NOT EXISTS idx_budget_categories_event ON convena.event_budget_categories (event_id);

        CREATE TABLE IF NOT EXISTS convena.event_expenses (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            event_id UUID NOT NULL REFERENCES convena.events(id) ON DELETE CASCADE,
            title VARCHAR(255) NOT NULL,
            category VARCHAR(50) NOT NULL,
            amount DOUBLE PRECISION NOT NULL DEFAULT 0,
            receipt_url TEXT,
            receipt_date TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            submitted_by_id UUID REFERENCES convena.users(id) ON DELETE SET NULL,
            submitted_by VARCHAR(255) NOT NULL,
            division VARCHAR(255) NOT NULL,
            spj_status VARCHAR(50) NOT NULL DEFAULT 'submitted',
            approved_by_id UUID REFERENCES convena.users(id) ON DELETE SET NULL,
            approved_by VARCHAR(255),
            notes TEXT,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );

        CREATE INDEX IF NOT EXISTS idx_expenses_event ON convena.event_expenses (event_id);
        CREATE INDEX IF NOT EXISTS idx_expenses_category ON convena.event_expenses (event_id, category);
        CREATE INDEX IF NOT EXISTS idx_expenses_status ON convena.event_expenses (event_id, spj_status);

        CREATE TABLE IF NOT EXISTS convena.organizations (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            name VARCHAR(255) NOT NULL,
            slug VARCHAR(255) UNIQUE NOT NULL,
            company VARCHAR(255),
            description TEXT,
            logo_url TEXT,
            banner_url TEXT,
            created_by UUID NOT NULL REFERENCES convena.users(id) ON DELETE CASCADE,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );

        CREATE INDEX IF NOT EXISTS idx_organizations_slug ON convena.organizations (slug);
        CREATE INDEX IF NOT EXISTS idx_organizations_created_by ON convena.organizations (created_by);

        CREATE TABLE IF NOT EXISTS convena.organization_roles (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            organization_id UUID NOT NULL REFERENCES convena.organizations(id) ON DELETE CASCADE,
            name VARCHAR(100) NOT NULL,
            display_name VARCHAR(100) NOT NULL,
            description TEXT,
            is_system BOOLEAN NOT NULL DEFAULT FALSE,
            permissions JSONB NOT NULL DEFAULT '[]'::jsonb,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            UNIQUE (organization_id, name)
        );

        CREATE INDEX IF NOT EXISTS idx_org_roles_org ON convena.organization_roles (organization_id);

        CREATE TABLE IF NOT EXISTS convena.organization_members (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            organization_id UUID NOT NULL REFERENCES convena.organizations(id) ON DELETE CASCADE,
            user_id UUID NOT NULL REFERENCES convena.users(id) ON DELETE CASCADE,
            role_id UUID NOT NULL REFERENCES convena.organization_roles(id) ON DELETE RESTRICT,
            custom_title VARCHAR(100),
            status VARCHAR(50) NOT NULL DEFAULT 'active',
            joined_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            UNIQUE (organization_id, user_id)
        );

        CREATE INDEX IF NOT EXISTS idx_org_members_user ON convena.organization_members (user_id);
        CREATE INDEX IF NOT EXISTS idx_org_members_org ON convena.organization_members (organization_id);
        CREATE INDEX IF NOT EXISTS idx_org_members_role ON convena.organization_members (role_id);

        DO $$
        BEGIN
            IF NOT EXISTS (
                SELECT 1 FROM information_schema.columns 
                WHERE table_schema = 'convena' AND table_name = 'events' AND column_name = 'organization_id'
            ) THEN
                ALTER TABLE convena.events ADD COLUMN organization_id UUID REFERENCES convena.organizations(id) ON DELETE SET NULL;
            END IF;

            ALTER TABLE convena.events ADD COLUMN IF NOT EXISTS latitude DOUBLE PRECISION;
            ALTER TABLE convena.events ADD COLUMN IF NOT EXISTS longitude DOUBLE PRECISION;
            ALTER TABLE convena.events ADD COLUMN IF NOT EXISTS attendance_qr_token VARCHAR(128);
            ALTER TABLE convena.event_members ADD COLUMN IF NOT EXISTS division VARCHAR(100) NOT NULL DEFAULT 'Umum';
            ALTER TABLE convena.event_invitations ADD COLUMN IF NOT EXISTS division VARCHAR(100) NOT NULL DEFAULT 'Umum';
        END $$;

        CREATE INDEX IF NOT EXISTS idx_events_organization_id ON convena.events (organization_id);

        CREATE TABLE IF NOT EXISTS convena.event_tasks (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            event_id UUID NOT NULL REFERENCES convena.events(id) ON DELETE CASCADE,
            title VARCHAR(255) NOT NULL,
            description TEXT NOT NULL DEFAULT '',
            assignee_id UUID REFERENCES convena.users(id) ON DELETE SET NULL,
            assignee_name VARCHAR(150) NOT NULL,
            division VARCHAR(100) NOT NULL DEFAULT 'Umum',
            due_date VARCHAR(32) NOT NULL,
            status VARCHAR(32) NOT NULL DEFAULT 'todo',
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
        );

        CREATE INDEX IF NOT EXISTS idx_event_tasks_event ON convena.event_tasks (event_id);

        CREATE TABLE IF NOT EXISTS convena.event_registrations (
            id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
            event_id UUID NOT NULL REFERENCES convena.events(id) ON DELETE CASCADE,
            user_id UUID NOT NULL REFERENCES convena.users(id) ON DELETE CASCADE,
            tier_id VARCHAR(100) NOT NULL,
            tier_name VARCHAR(150) NOT NULL,
            price DOUBLE PRECISION NOT NULL DEFAULT 0,
            payment_method VARCHAR(50) NOT NULL DEFAULT 'free',
            payment_status VARCHAR(32) NOT NULL DEFAULT 'paid',
            status VARCHAR(32) NOT NULL DEFAULT 'confirmed',
            attendee_name VARCHAR(150) NOT NULL,
            attendee_email VARCHAR(255) NOT NULL,
            attendee_phone VARCHAR(64),
            attendee_institution VARCHAR(150),
            ticket_code VARCHAR(64) UNIQUE NOT NULL,
            checked_in_at TIMESTAMPTZ,
            created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
            UNIQUE (event_id, user_id)
        );

        CREATE INDEX IF NOT EXISTS idx_event_registrations_event ON convena.event_registrations (event_id);
        CREATE INDEX IF NOT EXISTS idx_event_registrations_user ON convena.event_registrations (user_id);
    "#;

    sqlx::raw_sql(init_sql).execute(pool).await?;

    if let Err(e) = seed_dev_data(pool).await {
        tracing::warn!("Development data seeding skipped or encountered non-fatal issue: {e}");
    }

    tracing::info!("Database schema and tables initialized successfully.");
    Ok(())
}

/// Idempotently seed initial development data (users, events, memberships, event budgets & expenses).
pub async fn seed_dev_data(pool: &PgPool) -> Result<(), sqlx::Error> {
    let default_pw_hash = hash_password("password123")
        .unwrap_or_else(|_| "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$hash".to_string());

    let user_arya = Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let user_rina = Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap();
    let user_dimas = Uuid::parse_str("00000000-0000-0000-0000-000000000003").unwrap();

    // 1. Seed Dev Users
    let insert_user_sql = r#"
        INSERT INTO convena.users (id, display_name, username, email, password_hash, role, company)
        VALUES ($1, $2, $3, $4, $5, 'user', $6)
        ON CONFLICT (username) DO NOTHING
    "#;

    let dev_users = [
        (
            user_arya,
            "Arya Pratama",
            "arya",
            "arya@student.itb.ac.id",
            "Institut Teknologi Bandung",
        ),
        (
            user_rina,
            "Rina Kartika",
            "rina",
            "rina@student.itb.ac.id",
            "Institut Teknologi Bandung",
        ),
        (
            user_dimas,
            "Dimas Saputra",
            "dimas",
            "dimas@student.itb.ac.id",
            "Institut Teknologi Bandung",
        ),
    ];

    for (id, display_name, username, email, company) in dev_users {
        let _ = query(insert_user_sql)
            .bind(id)
            .bind(display_name)
            .bind(username)
            .bind(email)
            .bind(&default_pw_hash)
            .bind(company)
            .execute(pool)
            .await;
    }

    // Fetch actual user IDs in case username/email already existed with another UUID
    let arya_id: Uuid = query_scalar::<sqlx::Postgres, Uuid>(
        "SELECT id FROM convena.users WHERE username = 'arya' OR email = 'arya@student.itb.ac.id' LIMIT 1",
    )
    .fetch_optional(pool)
    .await?
    .unwrap_or(user_arya);

    let rina_id: Uuid = query_scalar::<sqlx::Postgres, Uuid>(
        "SELECT id FROM convena.users WHERE username = 'rina' OR email = 'rina@student.itb.ac.id' LIMIT 1",
    )
    .fetch_optional(pool)
    .await?
    .unwrap_or(user_rina);

    let dimas_id: Uuid = query_scalar::<sqlx::Postgres, Uuid>(
        "SELECT id FROM convena.users WHERE username = 'dimas' OR email = 'dimas@student.itb.ac.id' LIMIT 1",
    )
    .fetch_optional(pool)
    .await?
    .unwrap_or(user_dimas);

    // 2. Seed Dev Organizations & RBAC Roles/Members
    let org_hmif = Uuid::parse_str("33333333-3333-3333-3333-333333333301").unwrap();
    let org_telu = Uuid::parse_str("33333333-3333-3333-3333-333333333302").unwrap();

    let insert_org_sql = r#"
        INSERT INTO convena.organizations (
            id, name, slug, company, description, logo_url, banner_url, created_by
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
        ON CONFLICT (slug) DO NOTHING
    "#;

    let dev_orgs = [
        (
            org_hmif,
            "Himpunan Mahasiswa Informatika ITB",
            "hmif-itb",
            Some("Institut Teknologi Bandung"),
            Some("Organisasi mahasiswa program studi Teknik Informatika dan Sistem & Teknologi Informasi ITB yang mengoordinasikan kegiatan akademik, kepanitiaan festival teknologi, dan pengabdian masyarakat."),
            Some("https://images.unsplash.com/photo-1534447677768-be436bb09401?w=200&auto=format&fit=crop&q=80"),
            Some("https://images.unsplash.com/photo-1540575467063-178a50c2df87?w=1200&auto=format&fit=crop&q=80"),
            arya_id,
        ),
        (
            org_telu,
            "BEM Kema Telkom University",
            "bem-kema-tel-u",
            Some("Telkom University"),
            Some("Badan Eksekutif Mahasiswa Keluarga Mahasiswa Telkom University yang menaungi seluruh kegiatan kemahasiswaan terpadu, konferensi inovasi, dan advokasi mahasiswa."),
            Some("https://images.unsplash.com/photo-1523240795612-9a054b0db644?w=200&auto=format&fit=crop&q=80"),
            Some("https://images.unsplash.com/photo-1505373877841-8d25f7d46678?w=1200&auto=format&fit=crop&q=80"),
            arya_id,
        ),
    ];

    for (id, name, slug, company, description, logo_url, banner_url, created_by) in dev_orgs {
        let _ = query(insert_org_sql)
            .bind(id)
            .bind(name)
            .bind(slug)
            .bind(company)
            .bind(description)
            .bind(logo_url)
            .bind(banner_url)
            .bind(created_by)
            .execute(pool)
            .await;
    }

    // Provision default roles for each organization
    let insert_role_sql = r#"
        INSERT INTO convena.organization_roles (
            id, organization_id, name, display_name, description, is_system, permissions
        ) VALUES ($1, $2, $3, $4, $5, $6, $7)
        ON CONFLICT (organization_id, name) DO NOTHING
    "#;

    for org_id in [org_hmif, org_telu] {
        for (name, display_name, description, perms) in DEFAULT_SYSTEM_ROLES {
            let role_id = match *name {
                "owner" => {
                    if org_id == org_hmif {
                        Uuid::parse_str("44444444-4444-4444-4444-444444444101").unwrap()
                    } else {
                        Uuid::parse_str("44444444-4444-4444-4444-444444444201").unwrap()
                    }
                }
                "admin" => {
                    if org_id == org_hmif {
                        Uuid::parse_str("44444444-4444-4444-4444-444444444102").unwrap()
                    } else {
                        Uuid::parse_str("44444444-4444-4444-4444-444444444202").unwrap()
                    }
                }
                "treasurer" => {
                    if org_id == org_hmif {
                        Uuid::parse_str("44444444-4444-4444-4444-444444444103").unwrap()
                    } else {
                        Uuid::parse_str("44444444-4444-4444-4444-444444444203").unwrap()
                    }
                }
                "event_manager" => {
                    if org_id == org_hmif {
                        Uuid::parse_str("44444444-4444-4444-4444-444444444104").unwrap()
                    } else {
                        Uuid::parse_str("44444444-4444-4444-4444-444444444204").unwrap()
                    }
                }
                _ => {
                    if org_id == org_hmif {
                        Uuid::parse_str("44444444-4444-4444-4444-444444444105").unwrap()
                    } else {
                        Uuid::parse_str("44444444-4444-4444-4444-444444444205").unwrap()
                    }
                }
            };

            let perms_json = sqlx::types::Json(perms.to_vec());
            let _ = query(insert_role_sql)
                .bind(role_id)
                .bind(org_id)
                .bind(*name)
                .bind(*display_name)
                .bind(*description)
                .bind(true)
                .bind(perms_json)
                .execute(pool)
                .await;
        }
    }

    // Seed organization members
    let insert_org_member_sql = r#"
        INSERT INTO convena.organization_members (
            id, organization_id, user_id, role_id, custom_title, status
        ) VALUES ($1, $2, $3, $4, $5, 'active')
        ON CONFLICT (organization_id, user_id) DO NOTHING
    "#;

    let hmif_owner_role: Uuid = query_scalar::<sqlx::Postgres, Uuid>(
        "SELECT id FROM convena.organization_roles WHERE organization_id = $1 AND name = 'owner' LIMIT 1",
    )
    .bind(org_hmif)
    .fetch_optional(pool)
    .await?
    .unwrap_or_else(|| Uuid::parse_str("44444444-4444-4444-4444-444444444101").unwrap());

    let hmif_admin_role: Uuid = query_scalar::<sqlx::Postgres, Uuid>(
        "SELECT id FROM convena.organization_roles WHERE organization_id = $1 AND name = 'admin' LIMIT 1",
    )
    .bind(org_hmif)
    .fetch_optional(pool)
    .await?
    .unwrap_or_else(|| Uuid::parse_str("44444444-4444-4444-4444-444444444102").unwrap());

    let hmif_treasurer_role: Uuid = query_scalar::<sqlx::Postgres, Uuid>(
        "SELECT id FROM convena.organization_roles WHERE organization_id = $1 AND name = 'treasurer' LIMIT 1",
    )
    .bind(org_hmif)
    .fetch_optional(pool)
    .await?
    .unwrap_or_else(|| Uuid::parse_str("44444444-4444-4444-4444-444444444103").unwrap());

    let telu_owner_role: Uuid = query_scalar::<sqlx::Postgres, Uuid>(
        "SELECT id FROM convena.organization_roles WHERE organization_id = $1 AND name = 'owner' LIMIT 1",
    )
    .bind(org_telu)
    .fetch_optional(pool)
    .await?
    .unwrap_or_else(|| Uuid::parse_str("44444444-4444-4444-4444-444444444201").unwrap());

    // HMIF members
    let _ = query(insert_org_member_sql)
        .bind(Uuid::new_v4())
        .bind(org_hmif)
        .bind(arya_id)
        .bind(hmif_owner_role)
        .bind(Some("Ketua Umum HMIF"))
        .execute(pool)
        .await;

    let _ = query(insert_org_member_sql)
        .bind(Uuid::new_v4())
        .bind(org_hmif)
        .bind(rina_id)
        .bind(hmif_admin_role)
        .bind(Some("Sekretaris Jenderal"))
        .execute(pool)
        .await;

    let _ = query(insert_org_member_sql)
        .bind(Uuid::new_v4())
        .bind(org_hmif)
        .bind(dimas_id)
        .bind(hmif_treasurer_role)
        .bind(Some("Bendahara Utama"))
        .execute(pool)
        .await;

    // Tel-U members
    let _ = query(insert_org_member_sql)
        .bind(Uuid::new_v4())
        .bind(org_telu)
        .bind(arya_id)
        .bind(telu_owner_role)
        .bind(Some("Presiden Mahasiswa"))
        .execute(pool)
        .await;

    // 3. Seed Dev Events
    let evt_arkavidia = Uuid::parse_str("11111111-1111-1111-1111-111111111101").unwrap();
    let evt_summit = Uuid::parse_str("11111111-1111-1111-1111-111111111102").unwrap();
    let evt_ui_forum = Uuid::parse_str("11111111-1111-1111-1111-111111111103").unwrap();

    let arkavidia_tiers = serde_json::json!([
        {
            "id": "tkt-early",
            "name": "Early Bird Student Pass",
            "price": 35000.0,
            "quota": 200,
            "sold": 200,
            "benefits": ["Access to all main stages", "E-Certificate with verification ID", "Merchandise sticker pack", "Lunch & Coffee break"],
            "isAvailable": false
        },
        {
            "id": "tkt-regular",
            "name": "Regular Presale Pass",
            "price": 50000.0,
            "quota": 300,
            "sold": 220,
            "benefits": ["Access to all main stages & expo", "Official E-Certificate verified", "Free Workshop entrance voucher", "Networking lounge"],
            "isAvailable": true
        },
        {
            "id": "tkt-vip",
            "name": "VIP Hackathon Team Pass (3 Pax)",
            "price": 150000.0,
            "quota": 50,
            "sold": 38,
            "benefits": ["24-Hour Hackathon seat", "Dedicated mentor pairing", "Exclusive dinner with Judges & Tech VCs", "Swag kit + T-shirt"],
            "isAvailable": true
        }
    ]);

    let summit_tiers = serde_json::json!([
        {
            "id": "tkt-vip-workshop",
            "name": "VIP All-Access + Workshop",
            "price": 350000.0,
            "quota": 200,
            "sold": 142,
            "benefits": ["All-Stage VIP Access", "Hands-on AI Lab Workshop", "Exclusive Networking Dinner", "Verified Digital Certificate", "Official Summit Goodie Bag"],
            "isAvailable": true
        },
        {
            "id": "tkt-regular-summit",
            "name": "General Delegate Pass",
            "price": 150000.0,
            "quota": 400,
            "sold": 260,
            "benefits": ["Main Hall Access", "Digital Summit Certificate", "Coffee Break & Lunch"],
            "isAvailable": true
        }
    ]);

    let ui_tiers = serde_json::json!([
        {
            "id": "tkt-online",
            "name": "Virtual Attendee Pass",
            "price": 0.0,
            "quota": 500,
            "sold": 390,
            "benefits": ["High Definition Live Streaming", "Interactive Q&A Access", "Verified Digital E-Certificate"],
            "isAvailable": true
        },
        {
            "id": "tkt-onsite",
            "name": "Balairung Onsite Delegate",
            "price": 45000.0,
            "quota": 500,
            "sold": 390,
            "benefits": ["Priority Balairung Seating", "Networking Session", "Snack & Lunch Box", "Printed Delegate Card"],
            "isAvailable": true
        }
    ]);

    let insert_event_sql = r#"
        INSERT INTO convena.events (
            id, created_by, title, slug, description, short_description, banner_url,
            category, status, visibility, start_date, end_date, location_type,
            venue_name, address, city, online_url, organization_name, company, ticket_tiers,
            organization_id, latitude, longitude, attendance_qr_token
        ) VALUES (
            $1, $2, $3, $4, $5, $6, $7,
            $8, 'published', 'public', $9::timestamptz, $10::timestamptz, $11,
            $12, $13, $14, $15, $16, $17, $18,
            $19, $20, $21, $22
        )
        ON CONFLICT (slug) DO UPDATE SET
            organization_id = COALESCE(convena.events.organization_id, EXCLUDED.organization_id),
            latitude = COALESCE(convena.events.latitude, EXCLUDED.latitude),
            longitude = COALESCE(convena.events.longitude, EXCLUDED.longitude),
            attendance_qr_token = COALESCE(convena.events.attendance_qr_token, EXCLUDED.attendance_qr_token)
    "#;

    let _ = query(insert_event_sql)
        .bind(evt_arkavidia)
        .bind(arya_id)
        .bind("Arkavidia 9.0: National Tech Festival & Hackathon")
        .bind("arkavidia-9-national-tech-festival")
        .bind("Festival teknologi tahunan terbesar mahasiswa ITB yang menghadirkan Hackathon 24-jam, UI/UX Competition, seminar teknologi masa depan, dan pameran karya inovasi mahasiswa se-Indonesia.")
        .bind("National tech festival & hackathon organized by HMIF ITB with prizes up to Rp 50.000.000.")
        .bind("https://images.unsplash.com/photo-1540575467063-178a50c2df87?w=800&auto=format&fit=crop&q=80")
        .bind("festival")
        .bind("2026-11-14T09:00:00+07:00")
        .bind("2026-11-15T18:00:00+07:00")
        .bind("offline")
        .bind(Some("Aula Barat ITB"))
        .bind(Some("Jl. Ganesha No. 10, Bandung"))
        .bind(Some("Bandung"))
        .bind(None::<String>)
        .bind(Some("HMIF ITB"))
        .bind(Some("Institut Teknologi Bandung"))
        .bind(sqlx::types::Json(arkavidia_tiers))
        .bind(Some(org_hmif))
        .bind(Some(-6.8915_f64))
        .bind(Some(107.6107_f64))
        .bind(Some("ARKAV9-QR-2026"))
        .execute(pool)
        .await;

    let _ = query(insert_event_sql)
        .bind(evt_summit)
        .bind(arya_id)
        .bind("Global Tech Innovators Summit 2026 Tel-U")
        .bind("global-tech-innovators-summit-2026-tel-u")
        .bind("Konferensi internasional inovasi teknologi terdepan bersama para pakar Silicon Valley, pemimpin startup Asia, dan akademisi riset. Menghadirkan sesi keynote, workshop kecerdasan buatan, dan pameran prototipe masa depan.")
        .bind("Premier international tech summit featuring world-class keynotes, deep-dive workshops, and breakthrough innovations.")
        .bind("https://images.unsplash.com/photo-1505373877841-8d25f7d46678?w=800&auto=format&fit=crop&q=80")
        .bind("conference")
        .bind("2026-11-28T08:30:00+07:00")
        .bind("2026-11-29T17:00:00+07:00")
        .bind("offline")
        .bind(Some("Telkom University Convention Hall"))
        .bind(Some("Jl. Telekomunikasi No. 1, Buahbatu, Bandung"))
        .bind(Some("Bandung"))
        .bind(None::<String>)
        .bind(Some("BEM Kema Telkom University"))
        .bind(Some("Telkom University"))
        .bind(sqlx::types::Json(summit_tiers))
        .bind(Some(org_telu))
        .bind(Some(-6.9740_f64))
        .bind(Some(107.6305_f64))
        .bind(Some("TELUSUMMIT-QR-2026"))
        .execute(pool)
        .await;

    let _ = query(insert_event_sql)
        .bind(evt_ui_forum)
        .bind(arya_id)
        .bind("UI Youth Leadership & Policy Forum 2026")
        .bind("ui-youth-leadership-summit")
        .bind("Forum dialog kepemimpinan mahasiswa nasional bersama menteri, praktisi startup, dan penggerak sosial untuk merancang peta jalan Indonesia Berkelanjutan.")
        .bind("National leadership forum by BEM UI featuring youth leaders & changemakers across Indonesia.")
        .bind("https://images.unsplash.com/photo-1511578314322-379afb476865?w=800&auto=format&fit=crop&q=80")
        .bind("seminar")
        .bind("2026-11-20T08:30:00+07:00")
        .bind("2026-11-20T17:00:00+07:00")
        .bind("hybrid")
        .bind(Some("Balairung Universitas Indonesia"))
        .bind(Some("Kampus UI Depok, Jawa Barat"))
        .bind(Some("Depok"))
        .bind(Some("https://live.convena.id/ui-summit"))
        .bind(Some("BEM UI"))
        .bind(Some("Universitas Indonesia"))
        .bind(sqlx::types::Json(ui_tiers))
        .bind(Some(org_hmif))
        .bind(Some(-6.3686_f64))
        .bind(Some(106.8271_f64))
        .bind(Some("UIFORUM-QR-2026"))
        .execute(pool)
        .await;

    // 3. Seed Event Memberships for the dev events
    let insert_member_sql = r#"
        INSERT INTO convena.event_members (event_id, user_id, role, custom_title, division, status)
        VALUES ($1, $2, $3, $4, $5, 'active')
        ON CONFLICT (event_id, user_id) DO UPDATE SET
            division = CASE WHEN convena.event_members.division = 'Umum' THEN EXCLUDED.division ELSE convena.event_members.division END
    "#;

    for (evt_id, uid, role, title, division) in [
        (evt_arkavidia, arya_id, "owner", "Ketua Pelaksana", "BPH / Inti"),
        (evt_arkavidia, rina_id, "treasurer", "Bendahara Umum", "Divisi Keuangan"),
        (evt_arkavidia, dimas_id, "committee", "Koordinator Logistik", "Divisi Logistik"),
        (evt_summit, arya_id, "owner", "Project Director", "BPH / Inti"),
        (evt_summit, rina_id, "treasurer", "Head of Finance", "Divisi Keuangan"),
        (evt_ui_forum, arya_id, "owner", "Ketua Forum", "BPH / Inti"),
        (evt_ui_forum, rina_id, "treasurer", "Bendahara Forum", "Divisi Keuangan"),
    ] {
        let _ = query(insert_member_sql)
            .bind(evt_id)
            .bind(uid)
            .bind(role)
            .bind(title)
            .bind(division)
            .execute(pool)
            .await;
    }

    // Seed initial per-event committee tasks
    let insert_task_sql = r#"
        INSERT INTO convena.event_tasks (
            id, event_id, title, description, assignee_id, assignee_name, division, due_date, status
        ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
        ON CONFLICT (id) DO NOTHING
    "#;

    for (tid, eid, title, desc, aid, aname, div, due, status) in [
        (
            Uuid::parse_str("55555555-5555-5555-5555-555555555101").unwrap(),
            evt_arkavidia,
            "Finalisasi izin penggunaan Aula Barat & jaringan internet khusus Hackathon",
            "Koordinasi dengan Direktorat Sarana & Prasarana ITB serta DTI untuk bandwidth dedicated.",
            Some(dimas_id),
            "Dimas Saputra",
            "Divisi Logistik",
            "2026-11-05",
            "in_progress",
        ),
        (
            Uuid::parse_str("55555555-5555-5555-5555-555555555102").unwrap(),
            evt_arkavidia,
            "Verifikasi pencairan termin pertama sponsor utama & alokasi konsumsi",
            "Pastikan invoice termin 1 sudah masuk ke rekening kepanitiaan sebelum H-7.",
            Some(rina_id),
            "Rina Kartika",
            "Divisi Keuangan",
            "2026-11-07",
            "todo",
        ),
        (
            Uuid::parse_str("55555555-5555-5555-5555-555555555103").unwrap(),
            evt_arkavidia,
            "Kirim rundown final & teknis penjurian ke dewan juri Hackathon",
            "Rundown 24 jam dan rubrik penilaian inovasi wajib dikirim via email resmi.",
            Some(arya_id),
            "Arya Pratama",
            "Divisi Acara",
            "2026-11-01",
            "done",
        ),
        (
            Uuid::parse_str("55555555-5555-5555-5555-555555555201").unwrap(),
            evt_summit,
            "Konfirmasi akomodasi & penjemputan Keynote Speaker Bandara",
            "Pastikan LO speaker sudah standby sesuai jadwal kedatangan.",
            Some(arya_id),
            "Arya Pratama",
            "Divisi Acara",
            "2026-11-24",
            "in_progress",
        ),
    ] {
        let _ = query(insert_task_sql)
            .bind(tid)
            .bind(eid)
            .bind(title)
            .bind(desc)
            .bind(aid)
            .bind(aname)
            .bind(div)
            .bind(due)
            .bind(status)
            .execute(pool)
            .await;
    }

    // 4. Seed Event-Specific Budget Categories for Dev Events
    let insert_cat_sql = r#"
        INSERT INTO convena.event_budget_categories (event_id, category, label, planned_amount)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (event_id, category) DO NOTHING
    "#;

    let arkavidia_plans = [
        ("venue", "Sewa Gedung & Aula", 25_000_000.0),
        ("catering", "Konsumsi Peserta & Panitia", 18_000_000.0),
        ("equipment", "Sound, Lighting & Stage", 15_000_000.0),
        ("marketing", "Publikasi & Cetak Spanduk", 7_000_000.0),
        ("speaker_honor", "Honorarium Pembicara & Juri", 12_000_000.0),
        ("logistics", "Merchandise & Swag Kit", 8_000_000.0),
        ("other", "Biaya Lain-lain & Tak Terduga", 5_000_000.0),
    ];
    for (cat, label, planned) in arkavidia_plans {
        let _ = query(insert_cat_sql)
            .bind(evt_arkavidia)
            .bind(cat)
            .bind(label)
            .bind(planned)
            .execute(pool)
            .await;
    }

    let summit_plans = [
        ("venue", "Sewa Gedung & Aula", 35_000_000.0),
        ("catering", "Konsumsi Peserta & Panitia", 22_000_000.0),
        ("equipment", "Sound, Lighting & Stage", 20_000_000.0),
        ("marketing", "Publikasi & Cetak Spanduk", 10_000_000.0),
        ("speaker_honor", "Honorarium Pembicara & Juri", 25_000_000.0),
        ("logistics", "Merchandise & Swag Kit", 12_000_000.0),
        ("other", "Biaya Lain-lain & Tak Terduga", 6_000_000.0),
    ];
    for (cat, label, planned) in summit_plans {
        let _ = query(insert_cat_sql)
            .bind(evt_summit)
            .bind(cat)
            .bind(label)
            .bind(planned)
            .execute(pool)
            .await;
    }

    let ui_plans = [
        ("venue", "Sewa Gedung & Aula", 15_000_000.0),
        ("catering", "Konsumsi Peserta & Panitia", 12_000_000.0),
        ("equipment", "Sound, Lighting & Stage", 8_000_000.0),
        ("marketing", "Publikasi & Cetak Spanduk", 5_000_000.0),
        ("speaker_honor", "Honorarium Pembicara & Juri", 10_000_000.0),
        ("logistics", "Merchandise & Swag Kit", 6_000_000.0),
        ("other", "Biaya Lain-lain & Tak Terduga", 4_000_000.0),
    ];
    for (cat, label, planned) in ui_plans {
        let _ = query(insert_cat_sql)
            .bind(evt_ui_forum)
            .bind(cat)
            .bind(label)
            .bind(planned)
            .execute(pool)
            .await;
    }

    // Ensure all other existing events also have default RAB categories
    let all_event_ids: Vec<Uuid> =
        query_scalar::<sqlx::Postgres, Uuid>("SELECT id FROM convena.events")
            .fetch_all(pool)
            .await
            .unwrap_or_default();

    for event_id in all_event_ids {
        for (cat, label) in DEFAULT_BUDGET_CATEGORIES {
            let _ = query(insert_cat_sql)
                .bind(event_id)
                .bind(*cat)
                .bind(*label)
                .bind(0.0_f64)
                .execute(pool)
                .await;
        }
    }

    // 5. Seed Initial Event-Specific Expenses (using deterministic UUIDs so ON CONFLICT DO NOTHING works)
    let insert_exp_sql = r#"
        INSERT INTO convena.event_expenses (
            id, event_id, title, category, amount, receipt_date,
            submitted_by_id, submitted_by, division, spj_status,
            approved_by_id, approved_by, notes
        ) VALUES (
            $1, $2, $3, $4, $5, $6::timestamptz,
            $7, $8, $9, $10,
            $11, $12, $13
        )
        ON CONFLICT (id) DO NOTHING
    "#;

    let seeded_expenses = [
        (
            Uuid::parse_str("22222222-2222-2222-2222-222222222101").unwrap(),
            evt_arkavidia,
            "Down Payment Sewa Aula Barat ITB (50%)",
            "venue",
            12_500_000.0,
            "2026-10-10T09:00:00Z",
            Some(dimas_id),
            "Dimas Saputra (Logistik)",
            "Divisi Logistik",
            "verified",
            Some(rina_id),
            Some("Rina Kartika (Bendahara)"),
            Some("Kuitansi resmi SARPRAS ITB terlampir dengan materai Rp 10.000."),
        ),
        (
            Uuid::parse_str("22222222-2222-2222-2222-222222222102").unwrap(),
            evt_arkavidia,
            "Pelunasan Sewa Aula Barat ITB",
            "venue",
            12_500_000.0,
            "2026-10-15T14:30:00Z",
            Some(dimas_id),
            "Dimas Saputra (Logistik)",
            "Divisi Logistik",
            "verified",
            Some(rina_id),
            Some("Rina Kartika (Bendahara)"),
            Some("Pelunasan lunas, bukti potong pajak PPh 23 lengkap."),
        ),
        (
            Uuid::parse_str("22222222-2222-2222-2222-222222222103").unwrap(),
            evt_arkavidia,
            "Cetak 500 ID Card & Lanyard Peserta",
            "marketing",
            3_200_000.0,
            "2026-10-21T11:00:00Z",
            Some(arya_id),
            "Arya Pratama (Panitia)",
            "Divisi Acara & Kreatif",
            "submitted",
            None,
            None,
            Some("Invoice dari Percetakan Surya Grafika Bandung, menunggu verifikasi SPJ."),
        ),
        (
            Uuid::parse_str("22222222-2222-2222-2222-222222222104").unwrap(),
            evt_arkavidia,
            "Konsumsi Technical Meeting & Gladi Kotor",
            "catering",
            4_500_000.0,
            "2026-10-23T16:00:00Z",
            Some(dimas_id),
            "Dimas Saputra (Logistik)",
            "Divisi Konsumsi",
            "verified",
            Some(rina_id),
            Some("Rina Kartika (Bendahara)"),
            Some("Nota katering 150 box makan siang panitia dan pemateri."),
        ),
        (
            Uuid::parse_str("22222222-2222-2222-2222-222222222201").unwrap(),
            evt_summit,
            "Booking Telkom University Convention Hall",
            "venue",
            20_000_000.0,
            "2026-10-12T10:00:00Z",
            Some(arya_id),
            "Arya Pratama (Project Director)",
            "Divisi Venue & Operasional",
            "verified",
            Some(rina_id),
            Some("Rina Kartika (Finance)"),
            Some("Bukti transfer DP gedung TUCH terverifikasi."),
        ),
        (
            Uuid::parse_str("22222222-2222-2222-2222-222222222202").unwrap(),
            evt_summit,
            "Tiket & Akomodasi Keynote Speaker Internasional",
            "speaker_honor",
            9_500_000.0,
            "2026-10-25T13:15:00Z",
            Some(arya_id),
            "Arya Pratama (Project Director)",
            "Divisi Acara & Speaker Liaison",
            "submitted",
            None,
            None,
            Some("Invoice maskapai dan reservasi hotel 2 malam di Bandung."),
        ),
        (
            Uuid::parse_str("22222222-2222-2222-2222-222222222301").unwrap(),
            evt_ui_forum,
            "Sewa Peralatan Live Streaming Multi-Cam Balairung",
            "equipment",
            5_500_000.0,
            "2026-10-18T15:00:00Z",
            Some(arya_id),
            "Arya Pratama",
            "Divisi Produksi & Multimedia",
            "verified",
            Some(rina_id),
            Some("Rina Kartika"),
            Some("Kuitansi vendor broadcast 4 kamera HDMI + switcher."),
        ),
    ];

    for (
        id,
        event_id,
        title,
        category,
        amount,
        receipt_date,
        submitted_by_id,
        submitted_by,
        division,
        spj_status,
        approved_by_id,
        approved_by,
        notes,
    ) in seeded_expenses
    {
        let _ = query(insert_exp_sql)
            .bind(id)
            .bind(event_id)
            .bind(title)
            .bind(category)
            .bind(amount)
            .bind(receipt_date)
            .bind(submitted_by_id)
            .bind(submitted_by)
            .bind(division)
            .bind(spj_status)
            .bind(approved_by_id)
            .bind(approved_by)
            .bind(notes)
            .execute(pool)
            .await;
    }

    Ok(())
}
