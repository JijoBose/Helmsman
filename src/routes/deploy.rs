use std::{convert::Infallible, fs, time::Duration};

use askama::Template;
use axum::{
    extract::State,
    http::StatusCode,
    response::{
        Html, IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};
use serde_yaml::Value;

use crate::{
    kamal::{DeployLine, KamalError, KamalRunner},
    state::AppState,
};

use super::{AppError, render};

#[derive(Template)]
#[template(path = "deploy.html")]
struct DeployTemplate {
    active: &'static str,
    project_path: String,
    deploy_config_path: String,
    service: String,
    image: String,
    servers_summary: String,
    accessories_summary: String,
}

pub async fn page(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let project_path = state.project_path.lock().await.clone();
    let deploy_config_path = state.deploy_config_path.lock().await.clone();

    let (service, image, servers_summary, accessories_summary) =
        read_deploy_summary(&deploy_config_path);

    render(DeployTemplate {
        active: "deploy",
        project_path: project_path.display().to_string(),
        deploy_config_path: deploy_config_path.display().to_string(),
        service,
        image,
        servers_summary,
        accessories_summary,
    })
}

fn read_deploy_summary(config_path: &std::path::Path) -> (String, String, String, String) {
    let contents = match fs::read_to_string(config_path) {
        Ok(c) => c,
        Err(_) => return (String::new(), String::new(), String::new(), String::new()),
    };

    let value: Value = match serde_yaml::from_str(&contents) {
        Ok(v) => v,
        Err(_) => return (String::new(), String::new(), String::new(), String::new()),
    };

    let service = value
        .get("service")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let image = value
        .get("image")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let servers_count = value
        .get("servers")
        .and_then(Value::as_sequence)
        .map(|s| s.len())
        .unwrap_or(0);

    let servers_summary = if servers_count > 0 {
        format!("{} servers", servers_count)
    } else {
        "servers not listed".to_string()
    };

    let acc_count = value
        .get("accessories")
        .and_then(Value::as_mapping)
        .map(|m| m.len())
        .unwrap_or(0);

    let accessories_summary = if acc_count > 0 {
        format!("{} accessories", acc_count)
    } else {
        "no accessories".to_string()
    };

    (service, image, servers_summary, accessories_summary)
}

pub async fn start(State(state): State<AppState>) -> Result<Response, AppError> {
    let runner = KamalRunner::new(state);

    match runner.start_deploy().await {
        Ok(run_id) => Ok((
            StatusCode::OK,
            Html(format!(
                r#"<span class="text-emerald-700">Deploy #{run_id} started.</span>"#
            )),
        )
            .into_response()),
        Err(KamalError::AlreadyRunning) => Ok((
            StatusCode::CONFLICT,
            Html(r#"<span class="text-amber-700">A deploy is already running.</span>"#),
        )
            .into_response()),
        Err(error) => Err(error.into()),
    }
}

pub async fn abort(State(state): State<AppState>) -> Result<Response, AppError> {
    let runner = KamalRunner::new(state);

    if runner.abort().await? {
        Ok((
            StatusCode::OK,
            Html(r#"<span class="text-red-700">Abort requested.</span>"#),
        )
            .into_response())
    } else {
        Ok((
            StatusCode::OK,
            Html(r#"<span class="text-slate-600">No deploy is running.</span>"#),
        )
            .into_response())
    }
}

pub async fn stream(
    State(state): State<AppState>,
) -> Sse<impl futures_core::Stream<Item = Result<Event, Infallible>>> {
    let mut rx = state.deploy_tx.subscribe();

    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(line) if line.done => {
                    yield Ok(done_event(line));
                    break;
                }
                Ok(line) => {
                    yield Ok(message_event(line));
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    let line = DeployLine::new(format!("Skipped {skipped} log messages"));
                    yield Ok(message_event(line));
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    };

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

fn message_event(line: DeployLine) -> Event {
    Event::default().data(line.html_fragment())
}

fn done_event(line: DeployLine) -> Event {
    Event::default()
        .event("deploy-done")
        .data(line.html_fragment())
}
