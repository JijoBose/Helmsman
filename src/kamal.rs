use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{ExitStatus, Stdio},
    time::Instant,
};

use once_cell::sync::Lazy;
use regex::Regex;
use sqlx::SqlitePool;
use thiserror::Error;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    process::Command,
    task::JoinHandle,
    time::{Duration, sleep},
};

use crate::state::AppState;

const KAMAL_BASE_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin";

static ANSI_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\x1b\[[0-?]*[ -/]*[@-~]").expect("valid ANSI regex"));

#[derive(Debug, Error)]
pub enum KamalError {
    #[error("a deploy is already running")]
    AlreadyRunning,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

#[derive(Debug, Clone)]
pub struct DeployLine {
    pub text: String,
    pub phase: DeployPhase,
    pub done: bool,
}

impl DeployLine {
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let phase = detect_phase(&text);

        Self {
            text,
            phase,
            done: false,
        }
    }

    pub fn done(text: impl Into<String>) -> Self {
        let text = text.into();

        Self {
            text,
            phase: DeployPhase::Done,
            done: true,
        }
    }

    pub fn html_fragment(&self) -> String {
        format!(
            r#"<pre class="log-line {}">{}</pre>"#,
            self.phase.css_class(),
            escape_html(&self.text)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployPhase {
    Info,
    Building,
    Pushing,
    Deploying,
    HealthCheck,
    Done,
    Error,
}

impl DeployPhase {
    fn css_class(self) -> &'static str {
        match self {
            DeployPhase::Info => "log-info",
            DeployPhase::Building => "log-building",
            DeployPhase::Pushing => "log-pushing",
            DeployPhase::Deploying => "log-deploying",
            DeployPhase::HealthCheck => "log-health-check",
            DeployPhase::Done => "log-done",
            DeployPhase::Error => "log-error",
        }
    }
}

#[derive(Clone)]
pub struct KamalRunner {
    state: AppState,
}

impl KamalRunner {
    pub fn new(state: AppState) -> Self {
        Self { state }
    }

    pub async fn start_deploy(&self) -> Result<i64, KamalError> {
        let mut child_guard = self.state.active_child.lock().await;
        if child_guard.is_some() {
            return Err(KamalError::AlreadyRunning);
        }

        let project_path = self.state.project_path.lock().await.clone();
        let project_path_string = project_path.display().to_string();
        let started_at = Instant::now();

        let run_id = sqlx::query("INSERT INTO deploy_runs (project_path) VALUES (?)")
            .bind(&project_path_string)
            .execute(&self.state.db)
            .await?
            .last_insert_rowid();

        self.record_line(
            run_id,
            DeployLine::new(format!("Starting kamal deploy in {project_path_string}")),
        )
        .await;

        let mut command = kamal_command(&project_path);
        command.arg("deploy");
        command.stdout(Stdio::piped()).stderr(Stdio::piped());

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                let line = DeployLine {
                    text: format!("Failed to start kamal deploy: {err}"),
                    phase: DeployPhase::Error,
                    done: false,
                };
                self.record_line(run_id, line).await;
                finish_run(
                    &self.state.db,
                    run_id,
                    -1,
                    started_at.elapsed().as_secs() as i64,
                )
                .await?;
                let _ = self
                    .state
                    .deploy_tx
                    .send(DeployLine::done("Deploy process failed"));
                return Err(KamalError::Io(err));
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        *child_guard = Some(child);
        drop(child_guard);

        let stdout_task =
            stdout.map(|reader| spawn_output_reader(self.state.clone(), run_id, reader));
        let stderr_task =
            stderr.map(|reader| spawn_output_reader(self.state.clone(), run_id, reader));
        spawn_waiter(
            self.state.clone(),
            run_id,
            started_at,
            stdout_task,
            stderr_task,
        );

        Ok(run_id)
    }

    pub async fn abort(&self) -> Result<bool, KamalError> {
        let killed = {
            let mut child_guard = self.state.active_child.lock().await;
            match child_guard.as_mut() {
                Some(child) => {
                    child.start_kill()?;
                    true
                }
                None => false,
            }
        };

        if killed {
            let line = DeployLine {
                text: "Abort requested".to_string(),
                phase: DeployPhase::Error,
                done: false,
            };
            let _ = self.state.deploy_tx.send(line.clone());

            if let Some(run_id) = latest_running_run_id(&self.state.db).await? {
                append_log_line(&self.state.db, run_id, &line.text).await?;
            }
        }

        Ok(killed)
    }

    async fn record_line(&self, run_id: i64, line: DeployLine) {
        let _ = self.state.deploy_tx.send(line.clone());
        let _ = append_log_line(&self.state.db, run_id, &line.text).await;
    }
}

pub fn kamal_command(project_path: &Path) -> Command {
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

fn detect_phase(line: &str) -> DeployPhase {
    let lower = line.to_ascii_lowercase();

    if lower.contains("error")
        || lower.contains("failed")
        || lower.contains("failure")
        || lower.contains("fatal")
    {
        DeployPhase::Error
    } else if lower.contains("health") {
        DeployPhase::HealthCheck
    } else if lower.contains("build") || lower.contains("building") {
        DeployPhase::Building
    } else if lower.contains("push") || lower.contains("pushing") {
        DeployPhase::Pushing
    } else if lower.contains("deploy")
        || lower.contains("boot")
        || lower.contains("start")
        || lower.contains("restart")
    {
        DeployPhase::Deploying
    } else if lower.contains("done") || lower.contains("finished") || lower.contains("complete") {
        DeployPhase::Done
    } else {
        DeployPhase::Info
    }
}

fn spawn_output_reader<R>(state: AppState, run_id: i64, reader: R) -> JoinHandle<()>
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();

        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    let line = DeployLine::new(strip_ansi(&line));
                    let _ = state.deploy_tx.send(line.clone());
                    let _ = append_log_line(&state.db, run_id, &line.text).await;
                }
                Ok(None) => break,
                Err(err) => {
                    let line = DeployLine {
                        text: format!("Failed to read kamal output: {err}"),
                        phase: DeployPhase::Error,
                        done: false,
                    };
                    let _ = state.deploy_tx.send(line.clone());
                    let _ = append_log_line(&state.db, run_id, &line.text).await;
                    break;
                }
            }
        }
    })
}

fn spawn_waiter(
    state: AppState,
    run_id: i64,
    started_at: Instant,
    stdout_task: Option<JoinHandle<()>>,
    stderr_task: Option<JoinHandle<()>>,
) {
    tokio::spawn(async move {
        let status = wait_for_child(&state).await;

        if let Some(task) = stdout_task {
            let _ = task.await;
        }
        if let Some(task) = stderr_task {
            let _ = task.await;
        }

        match status {
            Ok(Some(status)) => finish_deploy(state, run_id, started_at, status).await,
            Ok(None) => {}
            Err(err) => {
                let line = DeployLine {
                    text: format!("Failed while waiting for kamal deploy: {err}"),
                    phase: DeployPhase::Error,
                    done: false,
                };
                let _ = state.deploy_tx.send(line.clone());
                let _ = append_log_line(&state.db, run_id, &line.text).await;
                let _ =
                    finish_run(&state.db, run_id, -1, started_at.elapsed().as_secs() as i64).await;
                let _ = state
                    .deploy_tx
                    .send(DeployLine::done("Deploy finished with an internal error"));
            }
        }
    });
}

async fn wait_for_child(state: &AppState) -> std::io::Result<Option<ExitStatus>> {
    loop {
        let maybe_status = {
            let mut child_guard = state.active_child.lock().await;
            match child_guard.as_mut() {
                Some(child) => match child.try_wait()? {
                    Some(status) => {
                        *child_guard = None;
                        Some(status)
                    }
                    None => None,
                },
                None => return Ok(None),
            }
        };

        if maybe_status.is_some() {
            return Ok(maybe_status);
        }

        sleep(Duration::from_millis(200)).await;
    }
}

async fn finish_deploy(state: AppState, run_id: i64, started_at: Instant, status: ExitStatus) {
    let exit_code = status.code().unwrap_or(-1);
    let duration_seconds = started_at.elapsed().as_secs() as i64;
    let summary = if status.success() {
        format!("Deploy finished successfully in {duration_seconds}s")
    } else {
        format!("Deploy exited with code {exit_code} after {duration_seconds}s")
    };

    let line = DeployLine {
        text: summary.clone(),
        phase: if status.success() {
            DeployPhase::Done
        } else {
            DeployPhase::Error
        },
        done: false,
    };

    let _ = state.deploy_tx.send(line.clone());
    let _ = append_log_line(&state.db, run_id, &line.text).await;
    let _ = finish_run(&state.db, run_id, exit_code, duration_seconds).await;
    let _ = state.deploy_tx.send(DeployLine::done(summary));
}

async fn append_log_line(pool: &SqlitePool, run_id: i64, line: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE deploy_runs SET log = log || ? || char(10) WHERE id = ?")
        .bind(line)
        .bind(run_id)
        .execute(pool)
        .await?;

    Ok(())
}

async fn finish_run(
    pool: &SqlitePool,
    run_id: i64,
    exit_code: i32,
    duration_seconds: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE deploy_runs SET exit_code = ?, duration_seconds = ? WHERE id = ?")
        .bind(exit_code)
        .bind(duration_seconds)
        .bind(run_id)
        .execute(pool)
        .await?;

    Ok(())
}

async fn latest_running_run_id(pool: &SqlitePool) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM deploy_runs WHERE exit_code IS NULL ORDER BY id DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
}

fn escape_html(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());

    for char in input.chars() {
        match char {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(char),
        }
    }

    escaped
}
