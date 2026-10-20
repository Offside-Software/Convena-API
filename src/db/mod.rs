use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use sqlx::PgPool;
use std::str::FromStr;
use std::time::Duration;

pub fn get_connect_options(database_url: &str) -> Result<PgConnectOptions, sqlx::Error> {
    let options = PgConnectOptions::from_str(database_url)?
        .statement_cache_capacity(0); // Essential for Supabase / PgBouncer / Supavisor pooler
    Ok
    (options)
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
        // Supabase Session Pooler has a globa per-project session limit (15 by default)
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
    let _ = sqlx::raw_sql("CREATE SCHEMA IF NOT EXISTS convena;")
        .execute(pool)
        .await?;

    tracing::info!("Database schema and tables initialized successfully.");
    Ok(())
}
