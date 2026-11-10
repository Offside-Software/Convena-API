use convena_api::{config::Config, db};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let config = Config::load();
    tracing::info!("Connecting to database for manual initialization...");

    let pool = db::create_pool(&config.database_url).await?;
    tracing::info!("Connected. Running schema initialization and seed data...");

    db::init_database(&pool).await?;
    tracing::info!("Database schema, tables, and seed data initialized successfully!");

    Ok(())
}
