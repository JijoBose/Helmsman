mod kamal;
mod routes;
mod state;

use std::{
    collections::VecDeque,
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tokio::sync::{Mutex, broadcast};
use tower_http::services::ServeDir;

use state::AppState;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let project_paths = detect_project_paths()?;
    let db = init_database().await?;
    let (deploy_tx, _) = broadcast::channel(1_000);

    let state = AppState {
        project_path: Arc::new(Mutex::new(project_paths.project_path)),
        deploy_config_path: Arc::new(Mutex::new(project_paths.deploy_config_path)),
        deploy_tx,
        active_child: Arc::new(Mutex::new(None)),
        db,
    };

    let app = routes::router(state).nest_service("/static", ServeDir::new("static"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;

    println!("Helmsman listening on http://127.0.0.1:3000");
    axum::serve(listener, app).await?;

    Ok(())
}

struct ProjectPaths {
    project_path: PathBuf,
    deploy_config_path: PathBuf,
}

fn detect_project_paths() -> Result<ProjectPaths, std::io::Error> {
    let cwd = env::current_dir()?;
    let deploy_config_path = find_deploy_yml(&cwd)?.unwrap_or_else(|| cwd.join("deploy.yml"));

    if deploy_config_path.exists() {
        println!("Using Kamal config {}", deploy_config_path.display());
    } else {
        eprintln!(
            "No deploy.yml found under {}; new config will be written to {}.",
            cwd.display(),
            deploy_config_path.display()
        );
    }

    Ok(ProjectPaths {
        project_path: cwd,
        deploy_config_path,
    })
}

fn find_deploy_yml(root: &Path) -> Result<Option<PathBuf>, std::io::Error> {
    let root_config = root.join("deploy.yml");
    if root_config.is_file() {
        return Ok(Some(root_config));
    }

    let conventional_config = root.join("config").join("deploy.yml");
    if conventional_config.is_file() {
        return Ok(Some(conventional_config));
    }

    let mut pending = VecDeque::from([root.to_path_buf()]);

    while let Some(dir) = pending.pop_front() {
        let mut entries = match fs::read_dir(&dir) {
            Ok(entries) => entries.collect::<Result<Vec<_>, _>>()?,
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => continue,
            Err(error) => return Err(error),
        };

        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            let path = entry.path();
            let file_type = entry.file_type()?;
            let file_name = entry.file_name();
            let file_name = file_name.to_string_lossy();

            if file_type.is_file() && file_name == "deploy.yml" {
                return Ok(Some(path));
            }

            if file_type.is_dir() && !should_skip_search_dir(&file_name) {
                pending.push_back(path);
            }
        }
    }

    Ok(None)
}

fn should_skip_search_dir(name: &str) -> bool {
    matches!(
        name,
        ".bundle"
            | ".git"
            | ".kamal"
            | ".next"
            | "coverage"
            | "log"
            | "node_modules"
            | "storage"
            | "target"
            | "tmp"
            | "vendor"
    )
}

async fn init_database() -> Result<sqlx::SqlitePool, sqlx::Error> {
    let options = SqliteConnectOptions::new()
        .filename("helmsman.sqlite")
        .create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS deploy_runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            started_at TEXT NOT NULL DEFAULT (datetime('now')),
            project_path TEXT NOT NULL,
            log TEXT NOT NULL DEFAULT '',
            exit_code INTEGER,
            duration_seconds INTEGER
        )",
    )
    .execute(&pool)
    .await?;

    Ok(pool)
}
