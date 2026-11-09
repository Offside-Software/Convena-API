use sqlx::PgPool;
use tokio::sync::broadcast;

use crate::{config::Config, models::event::AttendanceRealtimeEvent};

#[derive(Clone)]
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub attendance_tx: broadcast::Sender<AttendanceRealtimeEvent>,
}