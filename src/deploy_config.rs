use std::{fs, path::Path, time::SystemTime};

use serde_yaml::Value;

#[derive(Debug, Clone)]
pub struct DeploySummary {
    pub exists: bool,
    pub parse_error: Option<String>,
    pub service: String,
    pub image: String,
    pub image_short: String,
    pub hosts: Vec<String>,
    pub roles: Vec<String>,
    pub accessories: Vec<String>,
    pub registry: String,
    pub proxy_hosts: Vec<ProxyLink>,
    pub raw: String,
    pub line_count: usize,
    pub last_modified: String,
}

#[derive(Debug, Clone)]
pub struct ProxyLink {
    pub label: String,
    pub href: String,
}

impl DeploySummary {
    pub fn load(path: &Path) -> Self {
        let raw = fs::read_to_string(path).unwrap_or_default();
        let exists = path.is_file();
        let last_modified = fs::metadata(path)
            .and_then(|meta| meta.modified())
            .map(humanize_since)
            .unwrap_or_default();

        if raw.trim().is_empty() {
            return Self::empty(exists, raw, last_modified, None);
        }

        match serde_yaml::from_str::<Value>(&raw) {
            Ok(value) => Self::from_value(exists, raw, last_modified, &value),
            Err(error) => Self::empty(exists, raw, last_modified, Some(error.to_string())),
        }
    }

    pub fn accessory_names(path: &Path) -> Vec<String> {
        Self::load(path).accessories
    }

    fn empty(
        exists: bool,
        raw: String,
        last_modified: String,
        parse_error: Option<String>,
    ) -> Self {
        let line_count = count_lines(&raw);
        Self {
            exists,
            parse_error,
            service: String::new(),
            image: String::new(),
            image_short: String::new(),
            hosts: Vec::new(),
            roles: Vec::new(),
            accessories: Vec::new(),
            registry: String::new(),
            proxy_hosts: Vec::new(),
            raw,
            line_count,
            last_modified,
        }
    }

    fn from_value(exists: bool, raw: String, last_modified: String, value: &Value) -> Self {
        let service = text_at(value, &["service"]);
        let image = text_at(value, &["image"]);
        let mut hosts = Vec::new();
        let mut roles = Vec::new();

        if let Some(servers) = value.get("servers") {
            collect_servers(servers, &mut hosts, &mut roles);
        }

        let accessories = value
            .get("accessories")
            .and_then(Value::as_mapping)
            .map(|accessories| {
                accessories
                    .keys()
                    .filter_map(Value::as_str)
                    .map(ToOwned::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        let registry = registry_label(value.get("registry"));
        let proxy_hosts = proxy_links(value.get("proxy"));
        let line_count = count_lines(&raw);

        Self {
            exists,
            parse_error: None,
            service,
            image_short: shorten_image(&image),
            image,
            hosts,
            roles,
            accessories,
            registry,
            proxy_hosts,
            raw,
            line_count,
            last_modified,
        }
    }
}

fn count_lines(raw: &str) -> usize {
    if raw.is_empty() {
        0
    } else {
        raw.lines().count()
    }
}

fn text_at(value: &Value, path: &[&str]) -> String {
    let mut current = value;
    for key in path {
        current = match current.get(*key) {
            Some(next) => next,
            None => return String::new(),
        };
    }
    current.as_str().unwrap_or("").trim().to_string()
}

fn collect_servers(servers: &Value, hosts: &mut Vec<String>, roles: &mut Vec<String>) {
    match servers {
        Value::String(host) => push_unique(hosts, host),
        Value::Sequence(items) => {
            for item in items {
                if let Some(host) = item.as_str() {
                    push_unique(hosts, host);
                }
            }
        }
        Value::Mapping(roles_map) => {
            for (role, value) in roles_map {
                let Some(role) = role.as_str() else {
                    continue;
                };
                roles.push(role.to_string());
                collect_role_hosts(value, hosts);
            }
        }
        _ => {}
    }
}

fn collect_role_hosts(value: &Value, hosts: &mut Vec<String>) {
    match value {
        Value::String(host) => push_unique(hosts, host),
        Value::Sequence(items) => {
            for item in items {
                if let Some(host) = item.as_str() {
                    push_unique(hosts, host);
                }
            }
        }
        Value::Mapping(map) => {
            if let Some(nested) = map.get(Value::String("hosts".into())) {
                collect_role_hosts(nested, hosts);
            }
        }
        _ => {}
    }
}

fn push_unique(hosts: &mut Vec<String>, host: &str) {
    let host = host.trim();
    if host.is_empty() || hosts.iter().any(|existing| existing == host) {
        return;
    }
    hosts.push(host.to_string());
}

fn registry_label(registry: Option<&Value>) -> String {
    let Some(registry) = registry else {
        return String::new();
    };
    let server = registry
        .get("server")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let username = registry
        .get("username")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();

    match (server.is_empty(), username.is_empty()) {
        (true, true) => String::new(),
        (false, true) => server.to_string(),
        (true, false) => username.to_string(),
        (false, false) => format!("{username} on {server}"),
    }
}

fn proxy_links(proxy: Option<&Value>) -> Vec<ProxyLink> {
    let Some(proxy) = proxy else {
        return Vec::new();
    };
    if proxy.as_bool() == Some(false) {
        return Vec::new();
    }

    let https = proxy.get("ssl").and_then(Value::as_bool).unwrap_or(true);
    let mut labels = Vec::new();

    if let Some(host) = proxy.get("host").and_then(Value::as_str) {
        labels.push(host.to_string());
    }
    if let Some(hosts) = proxy.get("hosts").and_then(Value::as_sequence) {
        for host in hosts {
            if let Some(host) = host.as_str() {
                labels.push(host.to_string());
            }
        }
    }

    let mut links = Vec::new();
    for label in labels {
        let label = label.trim();
        if label.is_empty() || links.iter().any(|link: &ProxyLink| link.label == label) {
            continue;
        }
        let href = safe_proxy_href(label, https).unwrap_or_default();
        links.push(ProxyLink {
            label: label.to_string(),
            href,
        });
    }
    links
}

fn safe_proxy_href(host: &str, https: bool) -> Option<String> {
    let safe = !host.is_empty()
        && host.len() <= 253
        && !host.contains("://")
        && !host.contains('/')
        && host
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || matches!(char, '.' | '-' | ':'));

    if !safe {
        return None;
    }

    let scheme = if https { "https" } else { "http" };
    Some(format!("{scheme}://{host}"))
}

fn shorten_image(image: &str) -> String {
    if image.is_empty() {
        return String::new();
    }

    let (repo, tag) = image
        .rsplit_once(':')
        .filter(|(_, tag)| !tag.contains('/'))
        .unwrap_or((image, ""));

    let repo_short = repo
        .rsplit('/')
        .take(2)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("/");

    if tag.is_empty() {
        repo_short
    } else if tag.len() > 12 && tag.chars().all(|char| char.is_ascii_hexdigit()) {
        format!("{repo_short}:{}…", &tag[..12])
    } else {
        format!("{repo_short}:{tag}")
    }
}

fn humanize_since(time: SystemTime) -> String {
    let secs = match SystemTime::now().duration_since(time) {
        Ok(duration) => duration.as_secs(),
        Err(_) => return "just now".to_string(),
    };
    let plural = |count: u64| if count == 1 { "" } else { "s" };

    if secs < 45 {
        "just now".to_string()
    } else if secs < 3600 {
        let minutes = (secs + 30) / 60;
        format!("{minutes} minute{} ago", plural(minutes))
    } else if secs < 86_400 {
        let hours = secs / 3600;
        format!("{hours} hour{} ago", plural(hours))
    } else {
        let days = secs / 86_400;
        format!("{days} day{} ago", plural(days))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kamal2_servers_accessories_and_proxy() {
        let raw = r#"
service: harbor
image: ghcr.io/acme/harbor:04004a42fb7633b5afadd34d2bd1b5cb5fd58480
servers:
  web:
    - 192.0.2.10
    - 192.0.2.11
  job:
    hosts:
      - 192.0.2.10
accessories:
  db:
    image: postgres:16
    host: 192.0.2.10
  redis:
    image: redis:7
registry:
  server: ghcr.io
  username: acme
proxy:
  host: harbor.example.com
  ssl: true
"#;

        let summary = DeploySummary::from_value(
            true,
            raw.to_string(),
            String::new(),
            &serde_yaml::from_str(raw).unwrap(),
        );

        assert_eq!(summary.service, "harbor");
        assert_eq!(summary.image_short, "acme/harbor:04004a42fb76…");
        assert_eq!(summary.hosts, vec!["192.0.2.10", "192.0.2.11"]);
        assert_eq!(summary.roles, vec!["web", "job"]);
        assert_eq!(summary.accessories, vec!["db", "redis"]);
        assert_eq!(summary.registry, "acme on ghcr.io");
        assert_eq!(summary.proxy_hosts.len(), 1);
        assert_eq!(summary.proxy_hosts[0].label, "harbor.example.com");
        assert_eq!(summary.proxy_hosts[0].href, "https://harbor.example.com");
    }

    #[test]
    fn proxy_link_rejects_unsafe_hosts() {
        assert!(safe_proxy_href("javascript:alert(1)", true).is_none());
        assert!(safe_proxy_href("example.com/path", true).is_none());
        assert_eq!(
            safe_proxy_href("app.example.com", false).as_deref(),
            Some("http://app.example.com")
        );
    }

    #[test]
    fn invalid_yaml_keeps_the_raw_file() {
        let summary = DeploySummary::empty(
            true,
            "service: [\n".into(),
            "just now".into(),
            Some("bad".into()),
        );
        assert_eq!(summary.parse_error.as_deref(), Some("bad"));
        assert!(summary.service.is_empty());
        assert!(summary.raw.contains("service"));
    }
}
