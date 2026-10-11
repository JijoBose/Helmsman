pub mod activity;
pub mod config;
pub mod containers;
pub mod history;
pub mod logs;
pub mod overview;
pub mod requests;
pub mod servers;

use askama::Template;
use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};

use crate::{deploy_config::DeploySummary, kamal::KamalError, state::AppState};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(overview::page))
        .route("/overview/status", get(overview::status))
        .route("/deploy", get(redirect_home).post(rejected))
        .route("/deploy/stream", get(rejected))
        .route("/deploy/abort", post(rejected))
        .route("/servers", get(redirect_hosts))
        .route("/servers/status", get(containers::hosts_fragment))
        .route("/containers", get(containers::page))
        .route("/containers/status", get(containers::status_fragment))
        .route("/logs", get(logs::page))
        .route("/logs/recent", get(logs::recent))
        .route("/logs/stream", get(logs::stream))
        .route("/requests", get(requests::page))
        .route("/requests/list", get(requests::list))
        .route("/config", get(config::page).put(rejected))
        .route("/config/raw", get(redirect_config))
        .route("/activity", get(activity::page))
        .route("/activity/audit", get(activity::audit_fragment))
        .route("/activity/rollback", post(rejected))
        .route("/history", get(history::list))
        .route("/history/:id", get(history::detail))
        .with_state(state)
}

async fn redirect_home() -> Redirect {
    Redirect::to("/")
}

async fn redirect_hosts() -> Redirect {
    Redirect::to("/containers?view=hosts")
}

async fn redirect_config() -> Redirect {
    Redirect::to("/config")
}

async fn rejected(State(state): State<AppState>) -> Result<Response, AppError> {
    let chrome = chrome(&state, "overview").await;
    let body = RejectedTemplate { chrome }.render()?;
    Ok((StatusCode::FORBIDDEN, Html(body)).into_response())
}

pub async fn chrome(state: &AppState, active: &'static str) -> Chrome {
    let project_path = state.project_path.lock().await.clone();
    let config_path = state.deploy_config_path.lock().await.clone();
    let summary = DeploySummary::load(&config_path);
    let project_name = project_path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("project")
        .to_string();

    Chrome {
        active,
        project_name,
        project_path: project_path.display().to_string(),
        service: summary.service,
        version: env!("CARGO_PKG_VERSION"),
    }
}

#[derive(Clone)]
pub struct Chrome {
    pub active: &'static str,
    pub project_name: String,
    pub project_path: String,
    pub service: String,
    pub version: &'static str,
}

#[derive(Template)]
#[template(path = "rejected.html")]
struct RejectedTemplate {
    chrome: Chrome,
}

pub fn render<T: Template>(template: T) -> Result<Html<String>, AppError> {
    Ok(Html(template.render()?))
}

#[derive(Debug)]
pub struct AppError {
    status: StatusCode,
    message: String,
}

impl AppError {
    pub fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, self.message).into_response()
    }
}

impl From<askama::Error> for AppError {
    fn from(error: askama::Error) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
    }
}

impl From<sqlx::Error> for AppError {
    fn from(error: sqlx::Error) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
    }
}

impl From<std::io::Error> for AppError {
    fn from(error: std::io::Error) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
    }
}

impl From<serde_yaml::Error> for AppError {
    fn from(error: serde_yaml::Error) -> Self {
        Self::new(StatusCode::BAD_REQUEST, error.to_string())
    }
}

impl From<KamalError> for AppError {
    fn from(error: KamalError) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, error.user_message())
    }
}
