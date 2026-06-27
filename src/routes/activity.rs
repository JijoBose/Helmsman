use askama::Template;
use axum::{
    extract::State,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use sqlx::Row;

use crate::kamal::{kamal_command, strip_ansi};

use super::{AppError, render};

#[derive(Template)]
#[template(path = "activity.html")]
struct ActivityTemplate {
    active: &'static str,
    service: String,
    runs: Vec<RunSummary>,
    audit_lines: Vec<String>,
}

#[derive(Debug)]
struct RunSummary {
    id: i64,
    started_at: String,
    exit_code: String,
}

pub async fn page(State(state): State<crate::state::AppState>) -> Result<Html<String>, AppError> {
    let project_path = state.project_path.lock().await.clone();
    let deploy_config_path = state.deploy_config_path.lock().await.clone();

    let service = read_service(&deploy_config_path);

    let runs = fetch_recent_runs(&state).await?;
    let audit = fetch_audit(&project_path).await;

    render(ActivityTemplate {
        active: "activity",
        service,
        runs,
        audit_lines: audit,
    })
}

pub async fn audit_fragment(State(state): State<crate::state::AppState>) -> Result<Html<String>, AppError> {
    let project_path = state.project_path.lock().await.clone();
    let lines = fetch_audit(&project_path).await;
    // Simple preformatted response
    let html = lines.iter().map(|l| format!("<pre class=\"log-line\">{}</pre>", escape_html(l))).collect::<Vec<_>>().join("");
    Ok(Html(html))
}

pub async fn rollback(State(state): State<crate::state::AppState>) -> Result<Response, AppError> {
    let project_path = state.project_path.lock().await.clone();

    let mut cmd = kamal_command(&project_path);
    cmd.arg("rollback");

    let output = match cmd.output().await {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            strip_ansi(&s)
        }
        Err(e) => format!("Failed to run rollback: {}", e),
    };

    let msg = if output.to_lowercase().contains("error") || output.to_lowercase().contains("fail") {
        format!("<span class=\"text-red-400\">Rollback attempted. Output: {}</span>", escape_html(&output.lines().take(4).collect::<Vec<_>>().join(" ")))
    } else {
        "<span class=\"text-emerald-400\">Rollback command issued. Check containers / logs.</span>".to_string()
    };

    Ok((StatusCode::OK, Html(msg)).into_response())
}

async fn fetch_recent_runs(state: &crate::state::AppState) -> Result<Vec<RunSummary>, AppError> {
    let rows = sqlx::query(
        "SELECT id, started_at, exit_code FROM deploy_runs ORDER BY id DESC LIMIT 6",
    )
    .fetch_all(&state.db)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| RunSummary {
            id: row.get("id"),
            started_at: row.get("started_at"),
            exit_code: row.get::<Option<i64>, _>("exit_code").map_or_else(|| "—".to_string(), |c| c.to_string()),
        })
        .collect())
}

async fn fetch_audit(project_path: &std::path::Path) -> Vec<String> {
    let mut cmd = kamal_command(project_path);
    cmd.arg("audit");
    match cmd.output().await {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            strip_ansi(&s)
                .lines()
                .map(|l| l.to_string())
                .collect()
        }
        Err(_) => vec!["Unable to fetch kamal audit (is kamal installed and configured?)".to_string()],
    }
}

fn read_service(config_path: &std::path::Path) -> String {
    let contents = match std::fs::read_to_string(config_path) {
        Ok(c) => c,
        _ => return String::new(),
    };
    let value: serde_yaml::Value = match serde_yaml::from_str(&contents) {
        Ok(v) => v,
        _ => return String::new(),
    };
    value.get("service").and_then(serde_yaml::Value::as_str).unwrap_or("").to_string()
}

fn escape_html(input: &str) -> String {
    input.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}