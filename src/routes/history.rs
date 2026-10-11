use askama::Template;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Html,
};
use sqlx::Row;

use crate::state::AppState;

use super::{AppError, Chrome, render};

#[derive(Debug)]
pub struct RecentRun {
    pub id: i64,
    pub started_at: String,
    pub exit_code: String,
    pub duration: String,
}

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
    chrome: Chrome,
    runs: Vec<DeployRunSummary>,
    empty: bool,
}

#[derive(Template)]
#[template(path = "history_detail.html")]
struct HistoryDetailTemplate {
    chrome: Chrome,
    run: DeployRunDetail,
}

pub async fn recent_runs(
    pool: &sqlx::SqlitePool,
    limit: i64,
) -> Result<Vec<RecentRun>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, started_at, exit_code, duration_seconds
         FROM deploy_runs
         ORDER BY id DESC
         LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| RecentRun {
            id: row.get("id"),
            started_at: row.get("started_at"),
            exit_code: format_exit_code(row.get("exit_code")),
            duration: format_duration(row.get("duration_seconds")),
        })
        .collect())
}

pub async fn list(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let chrome = super::chrome(&state, "history").await;
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
        chrome,
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

    let chrome = super::chrome(&state, "history").await;

    render(HistoryDetailTemplate {
        chrome,
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
