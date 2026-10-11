use askama::Template;
use axum::{extract::State, response::Html};

use crate::{
    kamal::{self, KamalQuery},
    state::AppState,
};

use super::{AppError, Chrome, history::recent_runs, render};

#[derive(Template)]
#[template(path = "activity.html")]
struct ActivityTemplate {
    chrome: Chrome,
    runs: Vec<crate::routes::history::RecentRun>,
}

#[derive(Template)]
#[template(path = "audit_panel.html")]
struct AuditPanelTemplate {
    lines: Vec<String>,
    error: Option<String>,
}

pub async fn page(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let chrome = super::chrome(&state, "activity").await;
    let runs = recent_runs(&state.db, 6).await?;

    render(ActivityTemplate { chrome, runs })
}

pub async fn audit_fragment(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let project_path = state.project_path.lock().await.clone();
    let (lines, error) = fetch_audit(&project_path).await;
    render(AuditPanelTemplate { lines, error })
}

async fn fetch_audit(project_path: &std::path::Path) -> (Vec<String>, Option<String>) {
    match kamal::output(project_path, &KamalQuery::Audit).await {
        Ok(output) if output.success => {
            let lines = output
                .text
                .lines()
                .map(str::trim_end)
                .filter(|line| !line.trim().is_empty() && !kamal::is_kamal_progress(line))
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>();
            (lines, None)
        }
        Ok(output) => {
            let detail = kamal::failure_detail(&output.text);
            let message = if detail.is_empty() {
                "Couldn't read the audit log. Nothing was changed.".to_string()
            } else {
                format!("Couldn't read the audit log. Nothing was changed.\n{detail}")
            };
            (Vec::new(), Some(message))
        }
        Err(error) => (Vec::new(), Some(error.user_message())),
    }
}
