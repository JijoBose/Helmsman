use std::convert::Infallible;
use std::time::Duration;

use askama::Template;
use axum::{
    extract::{Query, State},
    response::{
        Html,
        sse::{Event, KeepAlive, Sse},
    },
};
use futures_core::Stream;

use crate::kamal::{kamal_command, strip_ansi};

use super::{AppError, render};

#[derive(Template)]
#[template(path = "logs.html")]
struct LogsTemplate {
    active: &'static str,
    service: String,
    lines: Vec<String>,
}

#[derive(serde::Deserialize)]
pub struct LogsQuery {
    tail: Option<u32>,
}

pub async fn page(
    State(state): State<crate::state::AppState>,
    Query(q): Query<LogsQuery>,
) -> Result<Html<String>, AppError> {
    let project_path = state.project_path.lock().await.clone();
    let deploy_config_path = state.deploy_config_path.lock().await.clone();

    let service = read_service(&deploy_config_path);

    // If tail query, or default fetch recent logs (200 lines)
    let line_count = q.tail.unwrap_or(150);
    let recent = fetch_recent_logs(&project_path, line_count).await;

    render(LogsTemplate {
        active: "logs",
        service,
        lines: recent,
    })
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
    value
        .get("service")
        .and_then(serde_yaml::Value::as_str)
        .unwrap_or("")
        .to_string()
}

async fn fetch_recent_logs(project_path: &std::path::Path, lines: u32) -> Vec<String> {
    let mut cmd = kamal_command(project_path);
    cmd.arg("app").arg("logs").arg("--lines").arg(lines.to_string());
    let output = match cmd.output().await {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).to_string();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            s
        }
        Err(e) => format!("Failed to run kamal app logs: {}", e),
    };
    strip_ansi(&output)
        .lines()
        .map(|l| l.to_string())
        .collect()
}

pub async fn stream(
    State(state): State<crate::state::AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let project_path = state.project_path.lock().await.clone();

    let stream = async_stream::stream! {
        let mut cmd = kamal_command(&project_path);
        cmd.arg("app").arg("logs").arg("--follow").arg("--lines").arg("100");
        cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                yield Ok(Event::default().data(format!("Failed to start logs: {}", e)));
                return;
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        if let Some(out) = stdout {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut reader = BufReader::new(out).lines();
            loop {
                match reader.next_line().await {
                    Ok(Some(line)) => {
                        let clean = strip_ansi(&line);
                        yield Ok(Event::default().data(clean));
                    }
                    Ok(None) => break,
                    Err(_) => break,
                }
            }
        }
        if let Some(err) = stderr {
            use tokio::io::{AsyncBufReadExt, BufReader};
            let mut reader = BufReader::new(err).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                let clean = strip_ansi(&line);
                if !clean.trim().is_empty() {
                    yield Ok(Event::default().data(clean));
                }
            }
        }

        let _ = child.kill().await;
    };

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(15))
            .text("keep-alive"),
    )
}