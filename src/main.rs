pub mod config;
pub mod appstate;
pub mod db;
pub mod models;
pub mod routes;
pub mod services;

use std::net::SocketAddr;

use crate::{appstate::AppState, config::Config};
use axum::Extension;
use tower_http::cors::CorsLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {

    // 1. Initiaalize logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| "convena_api=debug,tower_http=info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // 2. Load Configuration
    let config = Config::load();
    tracing::info!("Configuration loaded. Database target configured");

    // 3. Initialize Database Connection Pool
    let pool = match db::create_pool(&config.database_url).await {
        Ok(p) => {
            tracing::info!("Connected to PostgreSQL database pool successfully.");
            p
        }
        Err(e) => {
            tracing::warn!("Could not connect to PostgreSQL on startup: {e}");
            tracing::info!("Falling abck to offline/lazy pool initialization. Please check POSTGRESQL_DATABASE_URL.");
            let options = db::get_connect_options(&config.database_url)?;
            db::get_pool_options().connect_lazy_with(options)
        }
    };

    // 4. Setup CORS
    let cors = CorsLayer::new()
        .allow_origin(tower_http::cors::AllowOrigin::mirror_request())
        .allow_credentials(true)
        .allow_methods(tower_http::cors::AllowMethods::mirror_request())
        .allow_headers(tower_http::cors::AllowHeaders::mirror_request());

    let app_state = AppState {
        pool: pool.clone(),
        config: config.clone(),
    };

    let trace_layer = tower_http::trace::TraceLayer::new_for_http()
        .make_span_with(tower_http::trace::DefaultMakeSpan::new().level(tracing::Level::INFO))
        .on_request(tower_http::trace::DefaultOnRequest::new().level(tracing::Level::INFO))
        .on_response(tower_http::trace::DefaultOnResponse::new().level(tracing::Level::INFO))
        .on_failure(tower_http::trace::DefaultOnFailure::new().level(tracing::Level::ERROR));

    let app = routes::build_router(app_state)
        .layer(trace_layer)
        .layer(cors)
        .layer(Extension(pool));


    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!("Convena Rust Tokio server listening on http://localhost:{}", config.port);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    println!("Hello, world!");
    Ok(())
}
