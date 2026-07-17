//! Bony Build web monitor — architecture & change-impact dashboard.

mod architecture;
mod catalog;
mod features;
mod git;
mod impact;
mod workflow;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use clap::Parser;
use serde::Deserialize;
use serde_json::json;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tracing_subscriber::EnvFilter;

use crate::catalog::CatalogCache;

#[derive(Debug, Parser)]
#[command(name = "bony-monitor", about = "Bony Build architecture & change monitor")]
struct Args {
    /// Bind address (default 127.0.0.1:8787).
    #[arg(long, default_value = "127.0.0.1:8787")]
    bind: String,

    /// Repository root (default: discover from cwd / crate dir).
    #[arg(long)]
    repo: Option<PathBuf>,

    /// Max commits to load.
    #[arg(long, default_value_t = 80)]
    limit: usize,
}

#[derive(Clone)]
struct AppState {
    repo: PathBuf,
    limit: usize,
    static_dir: PathBuf,
    catalog: Arc<CatalogCache>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .init();

    let args = Args::parse();
    let start = args
        .repo
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let repo = git::find_repo_root(&start).or_else(|_| {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        git::find_repo_root(&manifest)
    })?;

    let static_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static");
    let docs_dir = repo.join("docs");
    let catalog = Arc::new(CatalogCache::new(repo.clone()));

    let state = Arc::new(AppState {
        repo: repo.clone(),
        limit: args.limit,
        static_dir: static_dir.clone(),
        catalog,
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/api/health", get(api_health))
        .route("/api/overview", get(api_overview))
        .route("/api/architecture", get(api_architecture))
        .route("/api/workflow", get(api_workflow))
        .route("/api/features", get(api_features))
        .route("/api/changes", get(api_changes))
        .route("/api/changes/{sha}", get(api_change_detail))
        .nest_service("/repo-docs", ServeDir::new(docs_dir))
        .nest_service("/static", ServeDir::new(static_dir))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr: SocketAddr = args.bind.parse()?;
    tracing::info!(%addr, repo = %repo.display(), "bony-monitor listening");
    println!("Bony Monitor → http://{addr}");
    println!("Repo          → {}", repo.display());
    println!("Catalog       → hot-reload features.toml + src scan on each API");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn api_health(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let snap = state.catalog.snapshot();
    Json(json!({
        "ok": true,
        "rules": snap.rules.len(),
        "discovered": snap.discovered.len(),
        "desktop_modules": snap.desktop_module_count,
        "loaded_at": format!("{:?}", snap.loaded_at),
    }))
}

async fn index(State(state): State<Arc<AppState>>) -> Response {
    let path = state.static_dir.join("index.html");
    match std::fs::read_to_string(path) {
        Ok(html) => Html(html).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("missing static/index.html: {e}"),
        )
            .into_response(),
    }
}

async fn api_overview(State(state): State<Arc<AppState>>) -> Result<Json<serde_json::Value>, ApiError> {
    let catalog = state.catalog.snapshot();
    let changes =
        git::list_changes(&state.repo, state.limit, &catalog).map_err(ApiError::from)?;
    let arch = architecture::overview(&catalog);
    let mut area_counts: std::collections::BTreeMap<String, u64> =
        std::collections::BTreeMap::new();
    let mut feature_counts: std::collections::BTreeMap<String, u64> =
        std::collections::BTreeMap::new();
    let mut add = 0u64;
    let mut del = 0u64;
    let mut touched_features = std::collections::BTreeSet::new();
    for c in &changes {
        add += u64::from(c.additions);
        del += u64::from(c.deletions);
        for tag in &c.impact.tags {
            *area_counts.entry(tag.clone()).or_default() += 1;
        }
        for f in &c.impact.features {
            *feature_counts.entry(f.id.clone()).or_default() += 1;
            touched_features.insert(f.id.clone());
        }
    }
    Ok(Json(json!({
        "repo": state.repo.display().to_string(),
        "commit_count": changes.len(),
        "additions": add,
        "deletions": del,
        "area_counts": area_counts,
        "feature_counts": feature_counts,
        "features_touched": touched_features.len(),
        "features_total": catalog.rules.len(),
        "discovered_modules": catalog.discovered.len(),
        "desktop_modules": catalog.desktop_module_count,
        "latest": changes.first(),
        "architecture_title": arch.title,
        "layer_count": arch.layers.len(),
        "refreshed": true,
    })))
}

async fn api_architecture(
    State(state): State<Arc<AppState>>,
) -> Json<architecture::ArchitectureOverview> {
    let catalog = state.catalog.snapshot();
    Json(architecture::overview(&catalog))
}

async fn api_workflow(
    State(state): State<Arc<AppState>>,
) -> Json<workflow::WorkflowOverview> {
    let catalog = state.catalog.snapshot();
    Json(workflow::overview(&catalog))
}

async fn api_features(
    State(state): State<Arc<AppState>>,
) -> Result<Json<features::FeaturesOverview>, ApiError> {
    let catalog = state.catalog.snapshot();
    let changes =
        git::list_changes(&state.repo, state.limit, &catalog).map_err(ApiError::from)?;
    Ok(Json(features::features_overview(&catalog, &changes)))
}

#[derive(Debug, Deserialize)]
struct ChangesQuery {
    limit: Option<usize>,
    tag: Option<String>,
}

async fn api_changes(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ChangesQuery>,
) -> Result<Json<Vec<git::ChangeEntry>>, ApiError> {
    let catalog = state.catalog.snapshot();
    let limit = q.limit.unwrap_or(state.limit).clamp(1, 300);
    let mut changes =
        git::list_changes(&state.repo, limit, &catalog).map_err(ApiError::from)?;
    if let Some(tag) = q.tag {
        changes.retain(|c| c.impact.tags.iter().any(|t| t == &tag));
    }
    Ok(Json(changes))
}

async fn api_change_detail(
    State(state): State<Arc<AppState>>,
    Path(sha): Path<String>,
) -> Result<Json<git::ChangeEntry>, ApiError> {
    let catalog = state.catalog.snapshot();
    let detail =
        git::change_detail(&state.repo, &sha, &catalog).map_err(ApiError::from)?;
    Ok(Json(detail))
}

struct ApiError(anyhow::Error);

impl From<anyhow::Error> for ApiError {
    fn from(value: anyhow::Error) -> Self {
        Self(value)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"error": self.0.to_string()})),
        )
            .into_response()
    }
}
