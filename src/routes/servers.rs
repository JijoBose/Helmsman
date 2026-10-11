use once_cell::sync::Lazy;
use regex::Regex;
use tokio::task::JoinSet;

use crate::{
    deploy_config::DeploySummary,
    kamal::{self, KamalQuery, strip_ansi},
};

#[derive(Debug, Clone)]
pub struct ServerStatus {
    pub hostname: String,
    pub container_count: usize,
    pub containers: Vec<ContainerStatus>,
}

#[derive(Debug, Clone)]
pub struct ContainerStatus {
    pub name: String,
    pub role: String,
    pub host: String,
    pub container_id: String,
    pub status_label: String,
    pub status_detail: String,
    pub status_class: String,
    pub image: String,
    pub ports: String,
    pub running_for: String,
}

#[derive(Debug, Clone)]
pub struct FleetReport {
    pub servers: Vec<ServerStatus>,
    pub error: Option<String>,
    pub warnings: Vec<String>,
    pub hosts: usize,
    pub total: usize,
    pub running: usize,
    pub stopped: usize,
    pub roles: Vec<String>,
}

pub async fn fetch_server_status(
    project_path: &std::path::Path,
    deploy_config_path: &std::path::Path,
) -> FleetReport {
    let mut jobs = vec![KamalQuery::ProxyDetails, KamalQuery::AppDetails];
    let mut errors = Vec::new();

    for name in DeploySummary::accessory_names(deploy_config_path) {
        match KamalQuery::accessory_details(&name) {
            Ok(query) => jobs.push(query),
            Err(error) => errors.push(error.user_message()),
        }
    }

    let mut set = JoinSet::new();
    for query in jobs {
        let project = project_path.to_path_buf();
        set.spawn(async move {
            let label = query.label();
            match kamal::output(&project, &query).await {
                Ok(output) if output.success => Ok(output.text),
                Ok(output) => Err(failed_status(&label, &output.text)),
                Err(error) => Err(error.user_message()),
            }
        });
    }

    let mut outputs = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(Ok(text)) => outputs.push(text),
            Ok(Err(error)) => errors.push(error),
            Err(error) => errors.push(format!("A status check stopped unexpectedly: {error}")),
        }
    }

    let servers = parse_app_details(&outputs.join("\n"));
    let errors = unique_strings(errors);
    let joined = summarize_failures(&errors);
    let (error, warnings) = if servers.is_empty() {
        let error = if joined.trim().is_empty() {
            None
        } else {
            Some(joined)
        };
        (error, Vec::new())
    } else {
        (None, errors)
    };
    let (hosts, total, running, stopped, roles) = compute_stats(&servers);

    FleetReport {
        servers,
        error,
        warnings,
        hosts,
        total,
        running,
        stopped,
        roles,
    }
}

fn failed_status(label: &str, output: &str) -> String {
    let detail = kamal::failure_detail(output);

    if detail.is_empty() {
        format!("{label} failed without any output.")
    } else {
        format!("{label} failed:\n{detail}")
    }
}

fn summarize_failures(errors: &[String]) -> String {
    if errors.is_empty() {
        return String::new();
    }

    if errors.iter().all(|error| is_reachability_failure(error)) {
        let reason = if errors
            .iter()
            .any(|error| error.contains("Connection refused"))
        {
            "The connection was refused."
        } else if errors
            .iter()
            .any(|error| error.to_ascii_lowercase().contains("timed out"))
        {
            "The connection timed out."
        } else if errors.iter().any(|error| {
            error.contains("Permission denied") || error.contains("Authentication failed")
        }) {
            "SSH authentication was refused."
        } else if errors.iter().any(|error| {
            error.contains("No route to host") || error.contains("Network is unreachable")
        }) {
            "There is no route to the servers."
        } else {
            "The SSH connection failed."
        };

        return format!("Couldn't reach the servers. Nothing was changed. {reason}");
    }

    errors.join("\n\n")
}

fn is_reachability_failure(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("connection refused")
        || lower.contains("timed out")
        || lower.contains("permission denied")
        || lower.contains("authentication failed")
        || lower.contains("connection reset")
        || lower.contains("no route to host")
        || lower.contains("network is unreachable")
        || lower.contains("could not resolve")
        || lower.contains("name or service not known")
}

fn unique_strings(values: Vec<String>) -> Vec<String> {
    let mut unique = Vec::new();
    for value in values {
        if !value.is_empty() && !unique.contains(&value) {
            unique.push(value);
        }
    }
    unique
}

fn compute_stats(servers: &[ServerStatus]) -> (usize, usize, usize, usize, Vec<String>) {
    let hosts = servers.len();
    let mut total = 0;
    let mut running = 0;
    let mut roles = Vec::new();

    for server in servers {
        total += server.container_count;
        for container in &server.containers {
            if container.status_label == "Running" {
                running += 1;
            }
            if !container.role.is_empty() && !roles.contains(&container.role) {
                roles.push(container.role.clone());
            }
        }
    }

    (hosts, total, running, total.saturating_sub(running), roles)
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
        let host = server.hostname.clone();
        for container in &mut server.containers {
            container.host = host.clone();
        }
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
    if let Some(index) = servers
        .iter()
        .position(|server| server.hostname == hostname)
    {
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
        host: String::new(),
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
        status
            .split_whitespace()
            .next()
            .unwrap_or("Unknown")
            .to_string()
    }
}

fn status_class(status: &str) -> String {
    match status {
        "Running" => "pill-success",
        "Stopped" => "pill-danger",
        "Restarting" => "pill-warning",
        _ => "pill-neutral",
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
        assert_eq!(servers[0].containers[0].host, "159.198.68.206");
        assert_eq!(servers[0].containers[1].role, "db");
        assert_eq!(servers[0].containers[1].status_class, "pill-success");
    }
}
