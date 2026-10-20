use sqlx::PgPool;

use crate::config::Config;
use crate::services::realtime::RealtimeHub;

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
}