use std::{convert::Infallible, time::Duration};

use askama::Template;
use axum::{
    extract::State,
    http::StatusCode,
    response::{
        Html, IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
};

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
}

pub async fn page(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let project_path = state.project_path.lock().await.display().to_string();
    let deploy_config_path = state.deploy_config_path.lock().await.display().to_string();

    render(DeployTemplate {
        active: "deploy",
        project_path,
        deploy_config_path,
    })
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
