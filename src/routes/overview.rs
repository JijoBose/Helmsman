use askama::Template;
use axum::{extract::State, response::Html};

use crate::{deploy_config::DeploySummary, state::AppState};

use super::{
    AppError, Chrome,
    history::recent_runs,
    render,
    servers::{ContainerStatus, FleetReport, fetch_server_status},
};

#[derive(Template)]
#[template(path = "overview.html")]
struct OverviewTemplate {
    chrome: Chrome,
    summary: DeploySummary,
    runs: Vec<crate::routes::history::RecentRun>,
}

#[derive(Template)]
#[template(path = "overview_status.html")]
struct OverviewStatusTemplate {
    tone: &'static str,
    kicker: &'static str,
    title: String,
    detail: String,
    hosts: usize,
    running: usize,
    stopped: usize,
    total: usize,
    preview_title: String,
    preview: Vec<ContainerStatus>,
    error: Option<String>,
    warnings: Vec<String>,
}

pub async fn page(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let chrome = super::chrome(&state, "overview").await;
    let config_path = state.deploy_config_path.lock().await.clone();
    let summary = DeploySummary::load(&config_path);
    let runs = recent_runs(&state.db, 5).await?;

    render(OverviewTemplate {
        chrome,
        summary,
        runs,
    })
}

pub async fn status(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let project_path = state.project_path.lock().await.clone();
    let config_path = state.deploy_config_path.lock().await.clone();
    let report = fetch_server_status(&project_path, &config_path).await;
    let (tone, kicker, title, detail) = health_copy(&report);
    let (preview_title, preview) = preview_containers(&report);

    render(OverviewStatusTemplate {
        tone,
        kicker,
        title,
        detail,
        hosts: report.hosts,
        running: report.running,
        stopped: report.stopped,
        total: report.total,
        preview_title,
        preview,
        error: report.error,
        warnings: report.warnings,
    })
}

fn health_copy(report: &FleetReport) -> (&'static str, &'static str, String, String) {
    if report.total == 0 {
        if report.error.is_some() {
            (
                "bad",
                "Unavailable",
                "Helmsman couldn't check the servers.".to_string(),
                "Nothing was changed.".to_string(),
            )
        } else {
            (
                "neutral",
                "No containers",
                "Nothing is running.".to_string(),
                "Kamal didn't report any app, proxy, or accessory containers.".to_string(),
            )
        }
    } else if report.stopped == 0 {
        let title = if report.running == 1 {
            "The only container is running.".to_string()
        } else {
            format!("All {} containers are running.", report.running)
        };
        let detail = if report.hosts == 1 {
            "On 1 host.".to_string()
        } else {
            format!("Across {} hosts.", report.hosts)
        };
        ("good", "Healthy", title, detail)
    } else {
        (
            "warn",
            "Needs a look",
            format!(
                "{} of {} containers are not running.",
                report.stopped, report.total
            ),
            "Open Containers to see which ones.".to_string(),
        )
    }
}

fn preview_containers(report: &FleetReport) -> (String, Vec<ContainerStatus>) {
    let mut unhealthy = Vec::new();
    let mut healthy = Vec::new();

    for server in &report.servers {
        for container in &server.containers {
            if container.status_label == "Running" {
                healthy.push(container.clone());
            } else {
                unhealthy.push(container.clone());
            }
        }
    }

    if !unhealthy.is_empty() {
        unhealthy.truncate(5);
        ("Not running".to_string(), unhealthy)
    } else {
        healthy.truncate(5);
        ("Running now".to_string(), healthy)
    }
}
