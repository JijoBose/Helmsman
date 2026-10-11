//! The only Kamal processes Helmsman is allowed to start.
//!
//! `KamalQuery` is the whole allowlist. Every process is built from that enum,
//! always ends with `--skip-hooks`, and never accepts a free-form argument list.
//! The flag has to come after the subcommand. Kamal treats a leading
//! `--skip-hooks` as "show help" and exits successfully.
//!
//! Kamal 2.12 still runs hooks when the flag is in the right place: the parent
//! command records it, and the subcommand keeps the default. Each process also
//! loads `skip_kamal_hooks.rb`, which makes hook lookup miss.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use once_cell::sync::Lazy;
use regex::Regex;
use thiserror::Error;
use tokio::{process::Command, time::timeout};

const KAMAL_BASE_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin";
const INSPECT_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_LOG_LINES: u32 = 1_000;

static ANSI_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").expect("valid ANSI regex"));

#[derive(Debug, Error)]
pub enum KamalError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0} took too long and was stopped")]
    TimedOut(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl KamalError {
    pub fn user_message(&self) -> String {
        match self {
            KamalError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
                "Helmsman couldn't find the kamal command. Start it from a terminal where `kamal version` works."
                    .to_string()
            }
            KamalError::Io(error) => format!("Could not run Kamal: {error}"),
            KamalError::TimedOut(command) => format!(
                "{command} took too long and was stopped. Check the SSH connection to your servers, then refresh."
            ),
            KamalError::Invalid(message) => message.clone(),
        }
    }
}

/// Read-only Kamal commands. Adding a variant is the only way to run something new.
#[derive(Debug, Clone)]
pub enum KamalQuery {
    AppDetails,
    ProxyDetails,
    AccessoryDetails { name: String },
    AppLogs { lines: u32, follow: bool },
    ProxyLogs { lines: u32 },
    Audit,
}

impl KamalQuery {
    pub fn accessory_details(name: impl Into<String>) -> Result<Self, KamalError> {
        let name = name.into();
        validate_accessory_name(&name)?;
        Ok(Self::AccessoryDetails { name })
    }

    pub fn app_logs(lines: u32, follow: bool) -> Self {
        Self::AppLogs { lines, follow }
    }

    pub fn proxy_logs(lines: u32) -> Self {
        Self::ProxyLogs { lines }
    }

    pub fn label(&self) -> String {
        match self.argv() {
            Ok(argv) => format!("kamal {}", argv.join(" ")),
            Err(_) => "kamal".to_string(),
        }
    }

    pub fn argv(&self) -> Result<Vec<String>, KamalError> {
        let mut argv = Vec::new();

        match self {
            KamalQuery::AppDetails => argv.extend(["app".into(), "details".into()]),
            KamalQuery::ProxyDetails => argv.extend(["proxy".into(), "details".into()]),
            KamalQuery::Audit => argv.push("audit".into()),
            KamalQuery::AccessoryDetails { name } => {
                validate_accessory_name(name)?;
                argv.extend(["accessory".into(), "details".into(), name.clone()]);
            }
            KamalQuery::AppLogs { lines, follow } => {
                argv.extend(["app".into(), "logs".into()]);
                if *follow {
                    argv.push("--follow".into());
                }
                argv.push("--lines".into());
                argv.push(clamp_lines(*lines).to_string());
            }
            KamalQuery::ProxyLogs { lines } => {
                argv.extend([
                    "proxy".into(),
                    "logs".into(),
                    "--lines".into(),
                    clamp_lines(*lines).to_string(),
                ]);
            }
        }

        // After the subcommand. A leading --skip-hooks makes Kamal print help.
        argv.push("--skip-hooks".into());
        Ok(argv)
    }
}

#[derive(Debug)]
pub struct CommandOutput {
    pub success: bool,
    pub text: String,
}

pub async fn output(project_path: &Path, query: &KamalQuery) -> Result<CommandOutput, KamalError> {
    let mut command = prepare(project_path, query)?;
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = command.spawn()?;
    let label = query.label();

    match timeout(INSPECT_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(CommandOutput {
            success: output.status.success(),
            text: combine_output(&output.stdout, &output.stderr),
        }),
        Ok(Err(error)) => Err(KamalError::Io(error)),
        Err(_) => Err(KamalError::TimedOut(label)),
    }
}

pub fn spawn(project_path: &Path, query: &KamalQuery) -> Result<tokio::process::Child, KamalError> {
    let mut command = prepare(project_path, query)?;
    command
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    Ok(command.spawn()?)
}

fn prepare(project_path: &Path, query: &KamalQuery) -> Result<Command, KamalError> {
    let mut command = kamal_binary(project_path);
    command.args(query.argv()?);
    command.env("RUBYOPT", rubyopt_with_hook_guard()?);
    Ok(command)
}

fn rubyopt_with_hook_guard() -> Result<String, KamalError> {
    let script = env::temp_dir().join("helmsman-skip-kamal-hooks.rb");
    fs::write(&script, include_str!("skip_kamal_hooks.rb"))?;

    let require = format!("-r{}", script.display());
    Ok(match env::var("RUBYOPT") {
        Ok(existing) if !existing.is_empty() => format!("{require} {existing}"),
        _ => require,
    })
}

fn kamal_binary(project_path: &Path) -> Command {
    let mut command = Command::new("kamal");
    let rvm_env = rvm_env_for_project(project_path);

    command
        .current_dir(project_path)
        .env("PATH", kamal_path(rvm_env.as_ref()));

    if let Some(rvm_env) = rvm_env {
        command
            .env("GEM_HOME", rvm_env.gem_home)
            .env("GEM_PATH", rvm_env.gem_path)
            .env("MY_RUBY_HOME", rvm_env.ruby_home);
    }

    if let Ok(home) = env::var("HOME") {
        command.env("HOME", home);
    }

    if let Ok(ssh_auth_sock) = env::var("SSH_AUTH_SOCK") {
        command.env("SSH_AUTH_SOCK", ssh_auth_sock);
    }

    command
}

fn validate_accessory_name(name: &str) -> Result<(), KamalError> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .next()
            .is_some_and(|char| char.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || char == '-' || char == '_');

    if valid {
        Ok(())
    } else {
        Err(KamalError::Invalid(format!(
            "Skipped accessory `{name}` because the name is not safe to pass to Kamal."
        )))
    }
}

fn clamp_lines(lines: u32) -> u32 {
    lines.clamp(1, MAX_LOG_LINES)
}

fn combine_output(stdout: &[u8], stderr: &[u8]) -> String {
    let mut combined = String::from_utf8_lossy(stdout).into_owned();
    combined.push_str(&String::from_utf8_lossy(stderr));
    strip_ansi(&combined)
}

struct RvmEnv {
    path_entries: Vec<PathBuf>,
    gem_home: PathBuf,
    gem_path: String,
    ruby_home: PathBuf,
}

fn kamal_path(rvm_env: Option<&RvmEnv>) -> String {
    let mut paths = Vec::new();

    if let Some(rvm_env) = rvm_env {
        paths.extend(
            rvm_env
                .path_entries
                .iter()
                .filter_map(|path| path.to_str().map(ToOwned::to_owned)),
        );
    }

    if let Ok(path) = env::var("PATH") {
        if !path.is_empty() {
            paths.push(path);
        }
    }

    paths.push(KAMAL_BASE_PATH.to_string());
    paths.join(":")
}

fn rvm_env_for_project(project_path: &Path) -> Option<RvmEnv> {
    let ruby_version = read_project_ruby_version(project_path)?;
    let (ruby_version, inline_gemset) = ruby_version
        .split_once('@')
        .map_or((ruby_version.as_str(), None), |(version, gemset)| {
            (version, Some(gemset))
        });

    let ruby_version = if ruby_version.starts_with("ruby-") {
        ruby_version.to_string()
    } else {
        format!("ruby-{ruby_version}")
    };

    let gemset = inline_gemset
        .map(ToOwned::to_owned)
        .or_else(|| read_project_ruby_gemset(project_path));
    let gem_home_name = gemset.filter(|gemset| !gemset.is_empty()).map_or_else(
        || ruby_version.clone(),
        |gemset| format!("{ruby_version}@{gemset}"),
    );

    let home = env::var_os("HOME").map(PathBuf::from)?;
    let rvm_root = home.join(".rvm");
    let ruby_home = rvm_root.join("rubies").join(&ruby_version);

    if !ruby_home.is_dir() {
        return None;
    }

    let gem_home = rvm_root.join("gems").join(gem_home_name);
    let global_gem_home = rvm_root.join("gems").join(format!("{ruby_version}@global"));
    let rvm_bin = rvm_root.join("bin");

    let path_entries = [
        gem_home.join("bin"),
        global_gem_home.join("bin"),
        ruby_home.join("bin"),
        rvm_bin,
    ]
    .into_iter()
    .filter(|path| path.is_dir())
    .collect::<Vec<_>>();

    let gem_path = [gem_home.clone(), global_gem_home]
        .into_iter()
        .filter_map(|path| path.to_str().map(ToOwned::to_owned))
        .collect::<Vec<_>>()
        .join(":");

    Some(RvmEnv {
        path_entries,
        gem_home,
        gem_path,
        ruby_home,
    })
}

fn read_project_ruby_version(project_path: &Path) -> Option<String> {
    read_project_file(project_path, ".ruby-version")
}

fn read_project_ruby_gemset(project_path: &Path) -> Option<String> {
    read_project_file(project_path, ".ruby-gemset")
}

fn read_project_file(project_path: &Path, name: &str) -> Option<String> {
    let value = fs::read_to_string(project_path.join(name)).ok()?;
    let value = value.trim();

    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

pub fn strip_ansi(input: &str) -> String {
    ANSI_RE.replace_all(input, "").to_string()
}

/// Kamal prints a progress line before every remote command. Those lines are
/// not application output, and on a failed connection they bury the reason.
pub fn failure_detail(text: &str) -> String {
    let mut kept = Vec::new();

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || is_kamal_progress(trimmed) {
            continue;
        }
        if !kept.iter().any(|seen: &String| seen == trimmed) {
            kept.push(trimmed.to_string());
        }
        if kept.len() == 6 {
            break;
        }
    }

    kept.join("\n")
}

pub fn is_kamal_progress(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("INFO [") && trimmed.contains("] Running ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_query_is_a_read_only_kamal_command() {
        let queries = [
            KamalQuery::AppDetails,
            KamalQuery::ProxyDetails,
            KamalQuery::Audit,
            KamalQuery::accessory_details("db").unwrap(),
            KamalQuery::app_logs(0, true),
            KamalQuery::app_logs(50_000, false),
            KamalQuery::proxy_logs(300),
        ];

        for query in queries {
            let argv = query.argv().unwrap();
            assert_eq!(
                argv.last().map(String::as_str),
                Some("--skip-hooks"),
                "hooks must stay off, and the flag must follow the subcommand"
            );

            let verb = match argv[0].as_str() {
                "audit" => "audit".to_string(),
                group => format!("{group} {}", argv[1]),
            };

            assert!(
                matches!(
                    verb.as_str(),
                    "app details"
                        | "app logs"
                        | "accessory details"
                        | "proxy details"
                        | "proxy logs"
                        | "audit"
                ),
                "unexpected kamal command: {verb}"
            );

            for arg in &argv {
                assert!(
                    !matches!(
                        arg.as_str(),
                        "deploy"
                            | "rollback"
                            | "redeploy"
                            | "reboot"
                            | "remove"
                            | "exec"
                            | "setup"
                            | "upgrade"
                            | "boot"
                            | "start"
                            | "stop"
                            | "push"
                            | "bootstrap"
                            | "prune"
                    ),
                    "mutating argument `{arg}` in {argv:?}"
                );
            }
        }
    }

    #[test]
    fn log_queries_only_carry_line_flags() {
        assert_eq!(
            KamalQuery::app_logs(50_000, true).argv().unwrap(),
            vec!["app", "logs", "--follow", "--lines", "1000", "--skip-hooks"]
        );
        assert_eq!(
            KamalQuery::app_logs(0, false).argv().unwrap(),
            vec!["app", "logs", "--lines", "1", "--skip-hooks"]
        );
        assert_eq!(
            KamalQuery::proxy_logs(300).argv().unwrap(),
            vec!["proxy", "logs", "--lines", "300", "--skip-hooks"]
        );
    }

    #[test]
    fn accessory_names_cannot_smuggle_flags_or_shell() {
        for name in [
            "--help",
            "-v",
            "db;rm",
            "db && true",
            "name with space",
            "",
            "../db",
            "a/b",
        ] {
            assert!(
                KamalQuery::accessory_details(name).is_err(),
                "accepted unsafe accessory name {name}"
            );
        }

        assert!(KamalQuery::accessory_details("redis_1").is_ok());
        assert!(KamalQuery::accessory_details("web-db").is_ok());
    }
}
