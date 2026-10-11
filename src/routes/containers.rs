use askama::Template;
use axum::{
    extract::{Query, State},
    response::Html,
};
use serde::Deserialize;

use crate::state::AppState;

use super::{AppError, Chrome, render, servers::fetch_server_status};

#[derive(Debug, Deserialize)]
pub struct ViewQuery {
    view: Option<String>,
}

#[derive(Template)]
#[template(path = "containers.html")]
struct ContainersTemplate {
    chrome: Chrome,
    view: String,
}

#[derive(Template)]
#[template(path = "fleet.html")]
struct FleetTemplate {
    view: String,
    empty: bool,
    error: Option<String>,
    warnings: Vec<String>,
    hosts: usize,
    total: usize,
    running: usize,
    stopped: usize,
    roles: Vec<String>,
    servers: Vec<crate::routes::servers::ServerStatus>,
}

pub async fn page(
    State(state): State<AppState>,
    Query(query): Query<ViewQuery>,
) -> Result<Html<String>, AppError> {
    let chrome = super::chrome(&state, "containers").await;
    render(ContainersTemplate {
        chrome,
        view: normalize_view(query.view.as_deref()),
    })
}

pub async fn hosts_fragment(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    render_fleet(state, "hosts").await
}

pub async fn status_fragment(
    State(state): State<AppState>,
    Query(query): Query<ViewQuery>,
) -> Result<Html<String>, AppError> {
    render_fleet(state, &normalize_view(query.view.as_deref())).await
}

async fn render_fleet(state: AppState, view: &str) -> Result<Html<String>, AppError> {
    let project_path = state.project_path.lock().await.clone();
    let config_path = state.deploy_config_path.lock().await.clone();
    let report = fetch_server_status(&project_path, &config_path).await;

    render(FleetTemplate {
        view: view.to_string(),
        empty: report.servers.is_empty(),
        error: report.error,
        warnings: report.warnings,
        hosts: report.hosts,
        total: report.total,
        running: report.running,
        stopped: report.stopped,
        roles: report.roles,
        servers: report.servers,
    })
}

fn normalize_view(view: Option<&str>) -> String {
    if view == Some("hosts") {
        "hosts".to_string()
    } else {
        "table".to_string()
    }
}
