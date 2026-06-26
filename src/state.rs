use std::{path::PathBuf, sync::Arc};

use sqlx::SqlitePool;
use tokio::{
    process::Child,
    sync::{Mutex, broadcast},
};

use crate::kamal::DeployLine;

#[derive(Clone)]
pub struct AppState {
    pub project_path: Arc<Mutex<PathBuf>>,
    pub deploy_config_path: Arc<Mutex<PathBuf>>,
    pub deploy_tx: broadcast::Sender<DeployLine>,
    pub active_child: Arc<Mutex<Option<Child>>>,
    pub db: SqlitePool,
}
