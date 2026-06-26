use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Html,
};
use sqlx::Row;

use crate::state::AppState;

use super::{AppError, render};

#[derive(Debug)]
struct DeployRunSummary {
    id: i64,
    started_at: String,
    project_path: String,
    exit_code: String,
    duration: String,
}

#[derive(Debug)]
struct DeployRunDetail {
    id: i64,
    started_at: String,
    project_path: String,
    exit_code: String,
    duration: String,
    log: String,
}

#[derive(Template)]
#[template(path = "history.html")]
struct HistoryTemplate {
    active: &'static str,
    runs: Vec<DeployRunSummary>,
    empty: bool,
}

#[derive(Template)]
#[template(path = "history_detail.html")]
struct HistoryDetailTemplate {
    active: &'static str,
    run: DeployRunDetail,
}

pub async fn list(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let rows = sqlx::query(
        "SELECT id, started_at, project_path, exit_code, duration_seconds
         FROM deploy_runs
         ORDER BY id DESC
         LIMIT 100",
    )
    .fetch_all(&state.db)
    .await?;

    let runs = rows
        .into_iter()
        .map(|row| DeployRunSummary {
            id: row.get("id"),
            started_at: row.get("started_at"),
            project_path: row.get("project_path"),
            exit_code: format_exit_code(row.get("exit_code")),
            duration: format_duration(row.get("duration_seconds")),
        })
        .collect::<Vec<_>>();

    render(HistoryTemplate {
        active: "history",
        empty: runs.is_empty(),
        runs,
    })
}

pub async fn detail(
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Result<Html<String>, AppError> {
    let row = sqlx::query(
        "SELECT id, started_at, project_path, exit_code, duration_seconds, log
         FROM deploy_runs
         WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "history run not found"))?;

    render(HistoryDetailTemplate {
        active: "history",
        run: DeployRunDetail {
            id: row.get("id"),
            started_at: row.get("started_at"),
            project_path: row.get("project_path"),
            exit_code: format_exit_code(row.get("exit_code")),
            duration: format_duration(row.get("duration_seconds")),
            log: row.get("log"),
        },
    })
}

fn format_exit_code(exit_code: Option<i64>) -> String {
    exit_code.map_or_else(|| "running".to_string(), |code| code.to_string())
}

fn format_duration(duration: Option<i64>) -> String {
    duration.map_or_else(|| "-".to_string(), |seconds| format!("{seconds}s"))
}
