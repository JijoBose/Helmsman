use std::{collections::BTreeMap, fs, time::SystemTime};

use askama::Template;
use axum::{Form, extract::State, response::Html};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

use super::{AppError, render};

#[derive(Debug, Deserialize)]
pub struct ConfigForm {
    service: String,
    image: String,
    servers: String,
    env_clear: String,
    env_secret: String,
    registry_server: String,
    registry_username: String,
    #[serde(default)]
    raw: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DeployConfig {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    service: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    image: String,
    #[serde(default)]
    servers: Vec<String>,
    #[serde(skip_serializing_if = "EnvConfig::is_empty", default)]
    env: EnvConfig,
    #[serde(skip_serializing_if = "RegistryConfig::is_empty", default)]
    registry: RegistryConfig,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct EnvConfig {
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    clear: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    secret: Vec<String>,
}

impl EnvConfig {
    fn is_empty(&self) -> bool {
        self.clear.is_empty() && self.secret.is_empty()
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryConfig {
    #[serde(skip_serializing_if = "String::is_empty", default)]
    server: String,
    #[serde(skip_serializing_if = "String::is_empty", default)]
    username: String,
}

impl RegistryConfig {
    fn is_empty(&self) -> bool {
        self.server.is_empty() && self.username.is_empty()
    }
}

#[derive(Template)]
#[template(path = "config.html")]
struct ConfigTemplate {
    active: &'static str,
    project_path: String,
    deploy_config_path: String,
    raw: String,
    last_modified: String,
    saved: bool,
    // legacy form fields kept for compatibility
    service: String,
    image: String,
    servers: String,
    env_clear: String,
    env_secret: String,
    registry_server: String,
    registry_username: String,
}

pub async fn page(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let project_path = state.project_path.lock().await.clone();
    let deploy_config_path = state.deploy_config_path.lock().await.clone();
    let (raw, last_modified) = read_raw(&deploy_config_path);
    let form = read_config_form(&deploy_config_path).unwrap_or_default();

    render(ConfigTemplate {
        active: "config",
        project_path: project_path.display().to_string(),
        deploy_config_path: deploy_config_path.display().to_string(),
        raw,
        last_modified,
        saved: false,
        service: form.service,
        image: form.image,
        servers: form.servers,
        env_clear: form.env_clear,
        env_secret: form.env_secret,
        registry_server: form.registry_server,
        registry_username: form.registry_username,
    })
}

pub async fn raw(State(state): State<AppState>) -> Result<Html<String>, AppError> {
    let deploy_config_path = state.deploy_config_path.lock().await.clone();
    let (raw, last_modified) = read_raw(&deploy_config_path);

    // Render a minimal snippet for htmx if needed, or full page. For now full with same template flag.
    render(ConfigTemplate {
        active: "config",
        project_path: "".to_string(),
        deploy_config_path: deploy_config_path.display().to_string(),
        raw,
        last_modified,
        saved: false,
        service: String::new(),
        image: String::new(),
        servers: String::new(),
        env_clear: String::new(),
        env_secret: String::new(),
        registry_server: String::new(),
        registry_username: String::new(),
    })
}

pub async fn save(
    State(state): State<AppState>,
    Form(form): Form<ConfigForm>,
) -> Result<Html<String>, AppError> {
    let project_path = state.project_path.lock().await.clone();
    let deploy_config_path = state.deploy_config_path.lock().await.clone();

    let content_to_write = if !form.raw.trim().is_empty() {
        form.raw.clone()
    } else {
        let config = DeployConfig {
            service: form.service.trim().to_string(),
            image: form.image.trim().to_string(),
            servers: lines(&form.servers),
            env: EnvConfig {
                clear: key_values(&form.env_clear),
                secret: lines(&form.env_secret),
            },
            registry: RegistryConfig {
                server: form.registry_server.trim().to_string(),
                username: form.registry_username.trim().to_string(),
            },
        };
        serde_yaml::to_string(&config)?
    };

    fs::write(&deploy_config_path, &content_to_write)?;

    let (raw, last_modified) = read_raw(&deploy_config_path);
    let form2 = read_config_form(&deploy_config_path).unwrap_or_default();

    render(ConfigTemplate {
        active: "config",
        project_path: project_path.display().to_string(),
        deploy_config_path: deploy_config_path.display().to_string(),
        raw,
        last_modified,
        saved: true,
        service: form2.service,
        image: form2.image,
        servers: form2.servers,
        env_clear: form2.env_clear,
        env_secret: form2.env_secret,
        registry_server: form2.registry_server,
        registry_username: form2.registry_username,
    })
}

impl Default for ConfigForm {
    fn default() -> Self {
        Self {
            service: String::new(),
            image: String::new(),
            servers: String::new(),
            env_clear: String::new(),
            env_secret: String::new(),
            registry_server: String::new(),
            registry_username: String::new(),
            raw: String::new(),
        }
    }
}

fn read_raw(path: &std::path::Path) -> (String, String) {
    let contents = fs::read_to_string(path).unwrap_or_default();
    let modified = fs::metadata(path)
        .and_then(|m| m.modified())
        .map(humanize_since)
        .unwrap_or_default();
    (contents, modified)
}

/// Render a file mtime as a compact relative time (std-only, no date dependency).
fn humanize_since(time: SystemTime) -> String {
    let secs = match SystemTime::now().duration_since(time) {
        Ok(d) => d.as_secs(),
        Err(_) => return "just now".to_string(),
    };
    let plural = |n: u64| if n == 1 { "" } else { "s" };
    if secs < 45 {
        "just now".to_string()
    } else if secs < 3600 {
        let m = (secs + 30) / 60;
        format!("{m} minute{} ago", plural(m))
    } else if secs < 86_400 {
        let h = secs / 3600;
        format!("{h} hour{} ago", plural(h))
    } else {
        let d = secs / 86_400;
        format!("{d} day{} ago", plural(d))
    }
}

fn read_config_form(path: &std::path::Path) -> Result<ConfigForm, AppError> {
    let contents = fs::read_to_string(path)?;
    let config = serde_yaml::from_str::<DeployConfig>(&contents)?;

    Ok(ConfigForm {
        service: config.service,
        image: config.image,
        servers: config.servers.join("\n"),
        env_clear: format_key_values(&config.env.clear),
        env_secret: config.env.secret.join("\n"),
        registry_server: config.registry.server,
        registry_username: config.registry.username,
        raw: String::new(),
    })
}

fn lines(input: &str) -> Vec<String> {
    input
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn key_values(input: &str) -> BTreeMap<String, String> {
    input
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .filter(|(key, _)| !key.is_empty())
        .collect()
}

fn format_key_values(values: &BTreeMap<String, String>) -> String {
    values
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
}
