use askama::Template;
use axum::{extract::State, response::Html};

use crate::{deploy_config::DeploySummary, state::AppState};

use super::{AppError, Chrome, render};

#[derive(Template)]
#[template(path = "config.html")]
struct ConfigTemplate {
    chrome: Chrome,
    summary: DeploySummary,
    config_path: String,
}

pub async fn page(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let chrome = super::chrome(&state, "config").await;
    let config_path = state.deploy_config_path.lock().await.clone();
    let summary = DeploySummary::load(&config_path);

    render(ConfigTemplate {
        chrome,
        summary,
        config_path: config_path.display().to_string(),
    })
}
