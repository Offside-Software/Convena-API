use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;
use std::str::FromStr;
use std::time::Duration;

pub fn get_connect_options(database_url: &str) -> Result<PgConnectOptions, sqlx::Error> {
    let options = PgConnectOptions::from_str(database_url)?
        .statement_cache_capacity(0); // Essential for Supabase / PgBouncer / Supavisor pooler
    Ok(options)
}

/// Help to construct queries with `.persistent(false)` for PgBouncer / Supavisor Compatibility.
/// Prevents named prepared statement collisions (`sqlx_s... alread exists`) in transaction pooling mode.
pub fn query<'q>(sql: &'q str) -> sqlx::query::Query<'q, sqlx::Postgres, <sqlx::Postgres as sqlx::Database>::Arguments<'q>> {
    sqlx::query(sql).persistent(false)
}

/// Helper to consturct query_as with `.persistent(false)` for PgBouncer / Supavisor compatibility.
pub fn query_as<'q, DB, O>(sql: &'q str) 
-> sqlx::query::QueryAs<'q, DB, O, <DB as sqlx::Database>::Arguments<'q>>
where
    DB: sqlx::Database + sqlx::database::HasStatementCache,
    O: for<'r> sqlx::FromRow<'r, DB::Row>,
{
    sqlx::query_as(sql).persistent(false)
}

/// Helper to consturct query_scalar with `.persistent(false)` for PgBouncer / Supavisor compatibility.
pub fn query_scalar<'q, DB, O>(sql: &'q str) 
-> sqlx::query::QueryScalar<'q, DB, O, <DB as sqlx::Database>::Arguments<'q>>
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

    let pool = get_pool_options()
        .connect_with(options)
        .await?;

    init_database(&pool).await?;
    Ok(pool)
}

pub async fn init_database(pool: &PgPool) -> Result<(), sqlx::Error> {
    // Ensure Convena Schema
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
    "#;

    sqlx::raw_sql(init_sql)
        .execute(pool)
        .await?;

    tracing::info!("Database schema and tables initialized successfully.");
    Ok(())
}
