use std::{convert::Infallible, time::Duration};

use askama::Template;
use axum::{
    extract::{Query, State},
    response::{
        Html,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures_core::Stream;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::{
    kamal::{self, KamalQuery, strip_ansi},
    state::AppState,
};

use super::{AppError, Chrome, render};

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    tail: Option<u32>,
}

#[derive(Template)]
#[template(path = "logs.html")]
struct LogsTemplate {
    chrome: Chrome,
    tail: u32,
}

#[derive(Template)]
#[template(path = "log_lines.html")]
struct LogLinesTemplate {
    lines: Vec<String>,
    error: Option<String>,
}

pub async fn page(
    State(state): State<AppState>,
    Query(query): Query<LogsQuery>,
) -> Result<Html<String>, AppError> {
    let chrome = super::chrome(&state, "logs").await;
    render(LogsTemplate {
        chrome,
        tail: normalize_tail(query.tail),
    })
}

pub async fn recent(
    State(state): State<AppState>,
    Query(query): Query<LogsQuery>,
) -> Result<Html<String>, AppError> {
    let project_path = state.project_path.lock().await.clone();
    let tail = normalize_tail(query.tail);
    let (lines, error) = read_logs(&project_path, tail).await;

    render(LogLinesTemplate { lines, error })
}

pub async fn stream(
    State(state): State<AppState>,
    Query(query): Query<LogsQuery>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let project_path = state.project_path.lock().await.clone();
    let tail = normalize_tail(query.tail);

    let stream = async_stream::stream! {
        let mut child = match kamal::spawn(&project_path, &KamalQuery::app_logs(tail, true)) {
            Ok(child) => child,
            Err(error) => {
                yield Ok(Event::default().data(error.user_message()));
                return;
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);

        if let Some(stdout) = stdout {
            let tx = tx.clone();
            tokio::spawn(async move { forward_lines(stdout, tx).await; });
        }
        if let Some(stderr) = stderr {
            let tx = tx.clone();
            tokio::spawn(async move { forward_lines(stderr, tx).await; });
        }
        drop(tx);

        while let Some(line) = rx.recv().await {
            yield Ok(Event::default().data(line));
        }

        drop(child);
    };

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}

async fn forward_lines<R>(reader: R, tx: tokio::sync::mpsc::Sender<String>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let clean = strip_ansi(&line);
        if clean.trim().is_empty() || kamal::is_kamal_progress(&clean) {
            continue;
        }
        if tx.send(clean).await.is_err() {
            break;
        }
    }
}

async fn read_logs(project_path: &std::path::Path, tail: u32) -> (Vec<String>, Option<String>) {
    match kamal::output(project_path, &KamalQuery::app_logs(tail, false)).await {
        Ok(output) if output.success => (tail_lines(&output.text, tail as usize), None),
        Ok(output) => {
            let detail = kamal::failure_detail(&output.text);
            let message = if detail.is_empty() {
                "Couldn't read the app logs. Nothing was changed.".to_string()
            } else {
                format!("Couldn't read the app logs. Nothing was changed.\n{detail}")
            };
            (Vec::new(), Some(message))
        }
        Err(error) => (Vec::new(), Some(error.user_message())),
    }
}

fn tail_lines(text: &str, max: usize) -> Vec<String> {
    let lines = text
        .lines()
        .map(str::trim_end)
        .filter(|line| !line.trim().is_empty() && !kamal::is_kamal_progress(line))
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();

    let skip = lines.len().saturating_sub(max);
    lines.into_iter().skip(skip).collect()
}

fn normalize_tail(tail: Option<u32>) -> u32 {
    tail.unwrap_or(200).clamp(1, 1_000)
}
