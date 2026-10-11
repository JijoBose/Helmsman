use askama::Template;
use axum::extract::State;
use axum::response::Html;

use crate::kamal::{self, KamalQuery};

use super::{AppError, Chrome, render};

#[derive(Template)]
#[template(path = "requests.html")]
struct RequestsTemplate {
    chrome: Chrome,
}

#[derive(Template)]
#[template(path = "requests_list.html")]
struct RequestsListTemplate {
    count: String,
    ok_2xx: String,
    three_xx: String,
    four_xx: String,
    five_xx: String,
    entries: Vec<RequestEntry>,
    empty: bool,
    error: Option<String>,
}

#[derive(Debug)]
pub struct RequestEntry {
    pub time: String,
    pub method: String,
    pub status: String,
    pub host: String,
    pub path: String,
}

pub async fn page(State(state): State<crate::state::AppState>) -> Result<Html<String>, AppError> {
    let chrome = super::chrome(&state, "requests").await;
    render(RequestsTemplate { chrome })
}

pub async fn list(State(state): State<crate::state::AppState>) -> Result<Html<String>, AppError> {
    let project_path = state.project_path.lock().await.clone();

    let fetched = match kamal::output(&project_path, &KamalQuery::proxy_logs(300)).await {
        Ok(output) if output.success => Ok(output.text),
        Ok(output) => Err({
            let detail = kamal::failure_detail(&output.text);
            if detail.is_empty() {
                "Couldn't read proxy logs. Nothing was changed.".to_string()
            } else {
                format!("Couldn't read proxy logs. Nothing was changed.\n{detail}")
            }
        }),
        Err(error) => Err(error.user_message()),
    };

    let (entries, error) = match fetched {
        Ok(log) => (parse_proxy_requests(&log), None),
        Err(message) => (Vec::new(), Some(message)),
    };

    let mut c2 = 0u32;
    let mut c3 = 0u32;
    let mut c4 = 0u32;
    let mut c5 = 0u32;
    for entry in &entries {
        if entry.status.starts_with('2') {
            c2 += 1;
        } else if entry.status.starts_with('3') {
            c3 += 1;
        } else if entry.status.starts_with('4') {
            c4 += 1;
        } else if entry.status.starts_with('5') {
            c5 += 1;
        }
    }

    render(RequestsListTemplate {
        count: entries.len().to_string(),
        ok_2xx: c2.to_string(),
        three_xx: c3.to_string(),
        four_xx: c4.to_string(),
        five_xx: c5.to_string(),
        empty: entries.is_empty(),
        entries,
        error,
    })
}

fn parse_proxy_requests(log: &str) -> Vec<RequestEntry> {
    let mut entries = Vec::new();
    for line in log.lines().rev().take(50) {
        // recent first
        let l = line.trim();
        if l.is_empty() {
            continue;
        }

        let mut method = "-".to_string();
        let mut status = "-".to_string();
        let mut host = "-".to_string();
        let mut path = "-".to_string();
        let mut time = extract_time(l).unwrap_or_else(|| "—".to_string());

        // Try to parse as JSON (common for modern proxy logs)
        if l.starts_with('{') {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(l) {
                if let Some(obj) = val.as_object() {
                    for (k, v) in obj {
                        let key = k.to_lowercase();
                        if key.contains("time") || key == "@timestamp" {
                            if let Some(s) = v.as_str() {
                                time = s.to_string();
                            }
                        }
                        if key.contains("method") || key == "verb" || key == "http_method" {
                            if let Some(s) = v.as_str() {
                                method = s.to_uppercase();
                            }
                        }
                        if key.contains("status") || key == "code" || key == "status_code" {
                            if let Some(n) = v.as_i64() {
                                status = n.to_string();
                            } else if let Some(s) = v.as_str() {
                                if s.len() <= 3 && s.chars().all(|c| c.is_ascii_digit()) {
                                    status = s.to_string();
                                }
                            }
                        }
                        if (key.contains("host") || key.contains("server_name")) && host == "-" {
                            if let Some(s) = v.as_str() {
                                host = s.to_string();
                            }
                        }
                        if (key.contains("path")
                            || key.contains("uri")
                            || key.contains("url")
                            || key == "request")
                            && path == "-"
                        {
                            if let Some(s) = v.as_str() {
                                path = s.to_string();
                            }
                        }
                    }
                }
                entries.push(RequestEntry {
                    time: shorten_time(&time),
                    method,
                    status,
                    host,
                    path,
                });
                continue;
            }
        }

        // Fallback heuristic for plain text access logs
        for token in l.split_whitespace() {
            let t = token.trim_matches(|c: char| {
                !c.is_alphanumeric() && c != '/' && c != '.' && c != ':' && c != '-'
            });
            if ["GET", "POST", "HEAD", "PUT", "DELETE", "PATCH", "OPTIONS"].contains(&t) {
                method = t.to_string();
            }
            if token.len() >= 3 && token.len() <= 3 && token.chars().all(|c| c.is_ascii_digit()) {
                let n: u16 = token.parse().unwrap_or(0);
                if (100..600).contains(&n) {
                    status = token.to_string();
                }
            }
            if host == "-"
                && (token.contains('.')
                    && (token.ends_with(".com") || token.ends_with(".io") || token.contains(':')))
            {
                host = token.to_string();
            }
        }

        // Extract path after seeing a method
        if let Some(pos) = l.find(" /") {
            let rest = &l[pos + 1..];
            let candidate = rest.split_whitespace().next().unwrap_or("");
            if candidate.starts_with('/') {
                path = candidate.to_string();
            }
        } else if path == "-" {
            // last resort, try to find any /path like thing
            if let Some(pos) = l.find('/') {
                let rest = &l[pos..];
                let candidate: String = rest
                    .chars()
                    .take_while(|&c| !c.is_whitespace() && c != '"')
                    .collect();
                if candidate.starts_with('/') {
                    path = candidate;
                }
            }
        }

        if path == "-" && !l.is_empty() {
            // don't set whole line
            path = l
                .split_whitespace()
                .find(|w| w.starts_with('/'))
                .unwrap_or("-")
                .to_string();
        }

        entries.push(RequestEntry {
            time: shorten_time(&time),
            method,
            status,
            host,
            path,
        });
    }
    entries
}

fn shorten_time(t: &str) -> String {
    // If it's an ISO timestamp, return just the time part
    if let Some(tpos) = t.find('T') {
        let after = &t[tpos + 1..];
        let cleaned = after
            .split(|c| c == '.' || c == '+' || c == 'Z')
            .next()
            .unwrap_or(after);
        if cleaned.contains(':') {
            return cleaned.to_string();
        }
    }
    if t.len() > 20 {
        return t.chars().take(19).collect();
    }
    t.to_string()
}

fn extract_time(line: &str) -> Option<String> {
    // Try to pull something that looks like time e.g. 3:31:30PM or 2026-...
    if let Some(start) = line.find(|c: char| c.is_ascii_digit()) {
        let candidate = &line[start..];
        let t: String = candidate.chars().take(20).collect();
        if t.contains(':') || t.contains('-') {
            return Some(t.split_whitespace().next().unwrap_or(&t).to_string());
        }
    }
    None
}
