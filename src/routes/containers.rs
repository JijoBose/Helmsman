use askama::Template;
use axum::{extract::State, response::Html};

use crate::{
    kamal::{kamal_command, strip_ansi},
    state::AppState,
};

use super::{AppError, render};
use super::servers::{fetch_server_status, ContainerStatus};

#[derive(Template)]
#[template(path = "containers.html")]
struct ContainersTemplate {
    active: &'static str,
    service: String,
    host: String,
    role: String,
    empty: bool,
    error: Option<String>,
    total_containers: usize,
    running_containers: usize,
    containers: Vec<ContainerStatus>,
    metrics: Vec<MetricCard>,
}

#[derive(Template)]
#[template(path = "containers_table.html")]
struct ContainersTableTemplate {
    empty: bool,
    error: Option<String>,
    containers: Vec<ContainerStatus>,
}

#[derive(Debug, Clone)]
struct MetricCard {
    icon: &'static str,
    label: &'static str,
    value: String,
    sub: String,
}

pub async fn page(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let project_path = state.project_path.lock().await.clone();
    let deploy_config_path = state.deploy_config_path.lock().await.clone();

    let (servers, error, _total_hosts, total_containers, running_containers) =
        fetch_server_status(&project_path, &deploy_config_path).await;

    let containers: Vec<ContainerStatus> = servers.into_iter().flat_map(|s| s.containers).collect();

    let (service, host) = read_service_and_host(&deploy_config_path);
    let metrics = fetch_metrics(&project_path).await;

    render(ContainersTemplate {
        active: "containers",
        service,
        host,
        role: "web".to_string(),
        empty: containers.is_empty(),
        error,
        total_containers,
        running_containers,
        containers: containers.clone(),
        metrics,
    })
}

pub async fn status_fragment(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let project_path = state.project_path.lock().await.clone();
    let deploy_config_path = state.deploy_config_path.lock().await.clone();
    let (servers, error, _total_hosts, _total, _running) =
        fetch_server_status(&project_path, &deploy_config_path).await;

    let containers: Vec<ContainerStatus> = servers.into_iter().flat_map(|s| s.containers).collect();

    render(ContainersTableTemplate {
        empty: containers.is_empty(),
        error,
        containers,
    })
}

fn read_service_and_host(config_path: &std::path::Path) -> (String, String) {
    let contents = match std::fs::read_to_string(config_path) {
        Ok(c) => c,
        _ => return (String::new(), String::new()),
    };
    let value: serde_yaml::Value = match serde_yaml::from_str(&contents) {
        Ok(v) => v,
        _ => return (String::new(), String::new()),
    };

    let service = value
        .get("service")
        .and_then(serde_yaml::Value::as_str)
        .unwrap_or("")
        .to_string();

    // Try to pick first web server host for display
    let host = value
        .get("servers")
        .and_then(|s| s.get("web"))
        .and_then(|v| if v.is_sequence() { v.as_sequence().and_then(|seq| seq.first()).and_then(|x| x.as_str()) } else { v.as_str() })
        .unwrap_or("")
        .to_string();

    (service, host)
}

async fn fetch_metrics(project_path: &std::path::Path) -> Vec<MetricCard> {
    // Try to get some basic stats via kamal server exec or local. Fall back gracefully.
    let mut cmd = kamal_command(project_path);
    // Use a lightweight command to get system info from primary server if possible.
    // We run "uptime && free -m 2>/dev/null | head -2 && df -h / 2>/dev/null | tail -1" via exec if servers defined.
    // Simpler: try docker stats once (local or on remote may vary), else return static-ish.
    cmd.arg("server").arg("exec").arg("uptime; free -m 2>/dev/null | head -2; df -h / 2>/dev/null | tail -1 || echo 'no df'");

    let output = match cmd.output().await {
        Ok(o) => strip_ansi(&String::from_utf8_lossy(&o.stdout)).to_string(),
        Err(_) => String::new(),
    };

    parse_metrics(&output)
}

fn parse_metrics(output: &str) -> Vec<MetricCard> {
    let mut cpu = "—".to_string();
    let mut ram = "—".to_string();
    let mut storage = "—".to_string();
    let mut load = "—".to_string();

    for line in output.lines() {
        let l = line.trim();
        if l.starts_with("load average") || l.contains("load average:") {
            // uptime line usually has "load average: 0.12, 0.34, 0.56"
            if let Some(idx) = l.find("load average:") {
                let rest = &l[idx + 13..];
                let parts: Vec<&str> = rest.split(',').map(|s| s.trim()).collect();
                if !parts.is_empty() {
                    load = format!("{}m", parts[0]);
                }
            }
        }
        if l.to_lowercase().starts_with("mem:") || (l.starts_with("total") && l.contains("used")) {
            // free -m header second line: Mem: total used free ...
            let nums: Vec<&str> = l.split_whitespace().filter(|x| x.chars().next().map_or(false, |c| c.is_ascii_digit())).collect();
            if nums.len() >= 2 {
                ram = format!("{:.0}%", (nums[1].parse::<f32>().unwrap_or(0.0) / nums[0].parse::<f32>().unwrap_or(1.0)) * 100.0 );
            }
        }
        if l.contains("%") && (l.contains("/dev/") || l.ends_with("G") || l.ends_with("M")) {
            // df line
            let cols: Vec<&str> = l.split_whitespace().collect();
            if cols.len() >= 5 {
                storage = cols[4].to_string(); // Use% 
                // Try better: show used / size
            }
        }
    }

    // Fallbacks if parsing failed
    if load == "—" {
        load = "1m 0.00".into();
    }
    if ram == "—" {
        ram = "—".into();
    }
    if storage == "—" {
        storage = "—".into();
    }
    if cpu == "—" {
        cpu = "—".into();
    }

    vec![
        // `icon` is an inline-SVG sprite id (see templates/icons.html), rendered via <use href="#i-{icon}">.
        MetricCard { icon: "cpu", label: "CPU", value: cpu, sub: "cores".into() },
        MetricCard { icon: "memory-stick", label: "RAM", value: ram, sub: "".into() },
        MetricCard { icon: "hard-drive", label: "Storage", value: storage, sub: "".into() },
        MetricCard { icon: "activity", label: "Load", value: load, sub: "".into() },
    ]
}