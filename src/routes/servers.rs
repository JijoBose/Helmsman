use askama::Template;
use axum::{extract::State, response::Html};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_yaml::Value;
use std::{fmt, fs, io::ErrorKind, path::Path};
use tokio::process::Command;

use crate::{
    kamal::{kamal_command, strip_ansi},
    state::AppState,
};

use super::{AppError, render};

#[derive(Debug, Clone)]
struct ServerStatus {
    hostname: String,
    container_count: usize,
    containers: Vec<ContainerStatus>,
}

#[derive(Debug, Clone)]
struct ContainerStatus {
    name: String,
    role: String,
    container_id: String,
    status_label: String,
    status_detail: String,
    status_class: String,
    image: String,
    ports: String,
    running_for: String,
}

#[derive(Template)]
#[template(path = "servers.html")]
struct ServersTemplate {
    servers: Vec<ServerStatus>,
    empty: bool,
    error: Option<String>,
}

pub async fn index(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let project_path = state.project_path.lock().await.clone();
    let deploy_config_path = state.deploy_config_path.lock().await.clone();
    let mut outputs = Vec::new();
    let mut errors = Vec::new();

    let mut command = kamal_command(&project_path);
    command.arg("app").arg("details");

    match run_status_command(command).await {
        Ok(output) => outputs.push(output),
        Err(error) => errors.push(format_status_error(&error)),
    }

    for accessory in accessory_names(&deploy_config_path) {
        let mut command = kamal_command(&project_path);
        command.arg("accessory").arg("details").arg(&accessory);

        match run_status_command(command).await {
            Ok(output) => outputs.push(output),
            Err(error) => {
                errors.push(format!("Accessory `{accessory}`: {}", format_status_error(&error)))
            }
        }
    }

    let servers = parse_app_details(&outputs.join("\n"));
    let error = if servers.is_empty() && !errors.is_empty() {
        Some(errors.join("\n\n"))
    } else {
        None
    };

    render(ServersTemplate {
        empty: servers.is_empty(),
        servers,
        error,
    })
}

enum StatusCommandError {
    Io(std::io::Error),
    Failed(String),
}

impl fmt::Display for StatusCommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StatusCommandError::Io(error) => write!(formatter, "{error}"),
            StatusCommandError::Failed(output) => formatter.write_str(output),
        }
    }
}

async fn run_status_command(mut command: Command) -> Result<String, StatusCommandError> {
    let output = command.output().await.map_err(StatusCommandError::Io)?;
    let mut combined = String::new();
    combined.push_str(&String::from_utf8_lossy(&output.stdout));
    combined.push_str(&String::from_utf8_lossy(&output.stderr));

    if !output.status.success() {
        return Err(StatusCommandError::Failed(format_failed_status(&combined)));
    }

    Ok(combined)
}

fn format_status_error(error: &StatusCommandError) -> String {
    match error {
        StatusCommandError::Io(error) if error.kind() == ErrorKind::NotFound => {
            "Could not find the `kamal` executable. Start Helmsman from a shell where `command -v kamal` works.".to_string()
        }
        _ => error.to_string(),
    }
}

fn format_failed_status(output: &str) -> String {
    let output = strip_ansi(output);
    let output = output.trim();

    if output.is_empty() {
        "kamal app details exited unsuccessfully without output.".to_string()
    } else {
        format!(
            "kamal app details failed:\n{}",
            output.lines().take(10).collect::<Vec<_>>().join("\n")
        )
    }
}

static DETAIL_COLUMNS_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\s{2,}").expect("valid details table regex"));

fn parse_app_details(output: &str) -> Vec<ServerStatus> {
    let mut servers = Vec::new();
    let mut current_host = None::<String>;

    for line in output.lines() {
        let clean = strip_ansi(line).trim().to_string();
        if clean.is_empty() || clean.starts_with("INFO ") || clean.starts_with("CONTAINER ID") {
            continue;
        }

        if let Some(host) = parse_host_line(&clean) {
            let index = server_index(&mut servers, &host);
            current_host = Some(servers[index].hostname.clone());
            continue;
        }

        let Some(host) = current_host.as_deref() else {
            continue;
        };

        if let Some(container) = parse_container_row(&clean) {
            let index = server_index(&mut servers, host);
            servers[index].containers.push(container);
        }
    }

    servers.retain(|server| !server.containers.is_empty());
    for server in &mut servers {
        server.container_count = server.containers.len();
    }

    servers
}

fn parse_host_line(line: &str) -> Option<String> {
    let (_, host) = line.split_once(" Host:")?;
    let host = host.trim();

    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

fn server_index(servers: &mut Vec<ServerStatus>, hostname: &str) -> usize {
    if let Some(index) = servers.iter().position(|server| server.hostname == hostname) {
        index
    } else {
        servers.push(ServerStatus {
            hostname: hostname.to_string(),
            container_count: 0,
            containers: Vec::new(),
        });
        servers.len() - 1
    }
}

fn parse_container_row(line: &str) -> Option<ContainerStatus> {
    let columns = DETAIL_COLUMNS_RE
        .split(line)
        .map(str::trim)
        .filter(|column| !column.is_empty())
        .collect::<Vec<_>>();

    if columns.len() < 7 || !looks_like_container_id(columns[0]) {
        return None;
    }

    let status_detail = columns[4].to_string();
    let status_label = status_label(&status_detail);

    Some(ContainerStatus {
        name: columns[6].to_string(),
        role: container_role(columns[6]),
        container_id: columns[0].to_string(),
        status_class: status_class(&status_label),
        status_label,
        running_for: running_for(&status_detail),
        status_detail,
        image: columns[1].to_string(),
        ports: columns[5].to_string(),
    })
}

fn looks_like_container_id(value: &str) -> bool {
    value.len() >= 8 && value.chars().all(|char| char.is_ascii_hexdigit())
}

fn status_label(status: &str) -> String {
    if status.starts_with("Up ") {
        "Running".to_string()
    } else if status.starts_with("Exited") {
        "Stopped".to_string()
    } else if status.starts_with("Restarting") {
        "Restarting".to_string()
    } else {
        status.split_whitespace().next().unwrap_or("Unknown").to_string()
    }
}

fn status_class(status: &str) -> String {
    match status {
        "Running" => "bg-emerald-100 text-emerald-800 ring-emerald-200",
        "Stopped" => "bg-red-100 text-red-800 ring-red-200",
        "Restarting" => "bg-amber-100 text-amber-800 ring-amber-200",
        _ => "bg-slate-100 text-slate-700 ring-slate-200",
    }
    .to_string()
}

fn running_for(status: &str) -> String {
    status
        .strip_prefix("Up ")
        .map_or_else(|| status.to_string(), ToOwned::to_owned)
}

fn container_role(name: &str) -> String {
    let parts = name.split('-').collect::<Vec<_>>();

    match parts.as_slice() {
        [] => "container".to_string(),
        [_] => name.to_string(),
        [_, role] => (*role).to_string(),
        _ => parts[parts.len() - 2].to_string(),
    }
}

fn accessory_names(deploy_config_path: &Path) -> Vec<String> {
    let Ok(contents) = fs::read_to_string(deploy_config_path) else {
        return Vec::new();
    };
    let Ok(config) = serde_yaml::from_str::<Value>(&contents) else {
        return Vec::new();
    };

    config
        .get("accessories")
        .and_then(Value::as_mapping)
        .map(|accessories| {
            accessories
                .keys()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_app_and_accessory_details_by_host() {
        let output = r#"
App Host: 159.198.68.206
CONTAINER ID   IMAGE                                                                COMMAND                  CREATED      STATUS      PORTS     NAMES
87c20f8a861e   ghcr.io/errsight/errsight:04004a42fb7633b5afadd34d2bd1b5cb5fd58480   "/rails/bin/docker"      4 days ago   Up 4 days   80/tcp    errsight-web-04004a42fb7633b5afadd34d2bd1b5cb5fd58480
Accessory db Host: 159.198.68.206
CONTAINER ID   IMAGE                               COMMAND                  CREATED       STATUS       PORTS                      NAMES
e9d1340c689b   timescale/timescaledb:latest-pg18   "docker-entrypoint.sh"   2 weeks ago   Up 2 weeks   127.0.0.1:5432->5432/tcp   errsight-db
"#;

        let servers = parse_app_details(output);

        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].hostname, "159.198.68.206");
        assert_eq!(servers[0].container_count, 2);
        assert_eq!(servers[0].containers[0].role, "web");
        assert_eq!(servers[0].containers[1].role, "db");
    }
}
