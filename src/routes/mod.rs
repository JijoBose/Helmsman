pub mod config;
pub mod deploy;
pub mod history;
pub mod servers;

use askama::Template;
use axum::{
    Router,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};

use crate::{kamal::KamalError, state::AppState};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(root))
        .route("/deploy", get(deploy::page).post(deploy::start))
        .route("/deploy/stream", get(deploy::stream))
        .route("/deploy/abort", post(deploy::abort))
        .route("/servers", get(servers::index))
        .route("/config", get(config::page).put(config::save))
        .route("/history", get(history::list))
        .route("/history/:id", get(history::detail))
        .with_state(state)
}

async fn root() -> Redirect {
    Redirect::to("/deploy")
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
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
    }
}
