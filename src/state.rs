use std::{path::PathBuf, sync::Arc};

use sqlx::SqlitePool;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct AppState {
    pub project_path: Arc<Mutex<PathBuf>>,
    pub deploy_config_path: Arc<Mutex<PathBuf>>,
    pub db: SqlitePool,
}
