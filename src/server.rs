use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::cache::{self, reader, schema};
use crate::delete::trash;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

struct AppState {
    #[allow(dead_code)]
    scan_root: PathBuf,
    db_path: PathBuf,
    /// Random secret token required for mutating operations (trash/delete).
    /// Embedded in the served HTML; prevents other tabs from calling the API.
    auth_token: String,
    /// Optional AI-generated insights (set via --insights flag or POST /api/insights)
    insights: Mutex<Option<InsightsData>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InsightsData {
    pub content: String,
}

// ---------------------------------------------------------------------------
// Query params
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct TreeParams {
    depth: Option<usize>,
}

#[derive(Deserialize)]
struct LargeFilesParams {
    min_size: Option<u64>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct OldFilesParams {
    days: Option<u64>,
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct TrashRequest {
    path: String,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn index(State(state): State<Arc<AppState>>) -> Html<String> {
    // Inject the auth token into the HTML so the page can authenticate its requests.
    // The token is only visible to this page — other origins cannot read it.
    let html = include_str!("templates/cleanup.html")
        .replace("__AUTH_TOKEN__", &state.auth_token);
    Html(html)
}

async fn api_info(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let conn = match open_db(&state.db_path) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    match reader::load_scan_meta(&conn) {
        Ok(Some(meta)) => Json(meta).into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "Not scanned").into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_tree(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TreeParams>,
) -> impl IntoResponse {
    let conn = match open_db(&state.db_path) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let depth = params.depth.unwrap_or(2);
    let root = match reader::load_root(&conn) {
        Ok(r) => r,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    match reader::load_tree_to_depth(&conn, root.id, depth) {
        Ok(tree) => Json(tree).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_large_files(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LargeFilesParams>,
) -> impl IntoResponse {
    let conn = match open_db(&state.db_path) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let min_size = params.min_size.unwrap_or(100_000_000); // 100 MB default
    let limit = params.limit.unwrap_or(50);
    match reader::query_large_files(&conn, min_size, limit) {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_dev_artifacts(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let conn = match open_db(&state.db_path) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    match reader::query_dev_artifacts(&conn) {
        Ok(rows) => {
            // Resolve full paths for dev artifacts
            let mut result: Vec<serde_json::Value> = Vec::new();
            for node in &rows {
                let full_path = reader::reconstruct_path(&conn, node.id)
                    .unwrap_or_else(|_| node.name.clone());
                let mut val = serde_json::to_value(node).unwrap_or_default();
                if let Some(obj) = val.as_object_mut() {
                    obj.insert("full_path".to_string(), serde_json::Value::String(full_path));
                }
                result.push(val);
            }
            Json(result).into_response()
        }
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_old_files(
    State(state): State<Arc<AppState>>,
    Query(params): Query<OldFilesParams>,
) -> impl IntoResponse {
    let conn = match open_db(&state.db_path) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let days = params.days.unwrap_or(365);
    let limit = params.limit.unwrap_or(50);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let cutoff = now - (days as i64 * 86400);
    match reader::query_old_files(&conn, cutoff, limit) {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_summary(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let conn = match open_db(&state.db_path) {
        Ok(c) => c,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    match reader::query_summary(&conn) {
        Ok(summary) => Json(summary).into_response(),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    }
}

async fn api_insights(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let guard = state.insights.lock().unwrap();
    match &*guard {
        Some(data) => Json(data.clone()).into_response(),
        None => Json(serde_json::json!(null)).into_response(),
    }
}

async fn api_set_insights(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<InsightsData>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }
    let mut guard = state.insights.lock().unwrap();
    *guard = Some(body);
    Json(serde_json::json!({"ok": true})).into_response()
}

/// Verify the auth token from the Authorization header.
fn check_auth(state: &AppState, headers: &HeaderMap) -> Result<(), (StatusCode, String)> {
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    match token {
        Some(t) if t == state.auth_token => Ok(()),
        _ => Err((StatusCode::FORBIDDEN, "Invalid or missing auth token".to_string())),
    }
}

async fn api_trash(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<TrashRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&state, &headers) {
        return e.into_response();
    }
    match trash::move_to_trash(&body.path) {
        Ok(result) => {
            // Remove trashed item from the cache so the UI stays in sync.
            if result.success {
                if let Ok(conn) = open_db(&state.db_path) {
                    let name = std::path::Path::new(&body.path)
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    // Try deleting as a file first, then as a directory
                    let _ = conn.execute(
                        "DELETE FROM files WHERE name = ?1 AND disk_size = ?2",
                        rusqlite::params![name, result.size_freed as i64],
                    );
                    let _ = conn.execute(
                        "DELETE FROM dirs WHERE name = ?1 AND total_disk_size = ?2",
                        rusqlite::params![name, result.size_freed as i64],
                    );
                }
            }
            Json(result).into_response()
        }
        Err(e) => {
            let result = trash::DeleteResult {
                path: body.path,
                size_freed: 0,
                success: false,
                error: Some(e.to_string()),
            };
            Json(result).into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn open_db(db_path: &Path) -> Result<rusqlite::Connection, String> {
    schema::open_db(db_path).map_err(|e| format!("Failed to open cache: {}", e))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Start the cleanup web server.
///
/// - `scan_root`: the path that was scanned (used for display + DB lookup)
/// - `port`: TCP port to listen on
/// - `insights_content`: optional AI-generated insights text
pub async fn serve(scan_root: PathBuf, port: u16, insights_content: Option<String>) -> anyhow::Result<()> {
    let db_path = cache::db_path_for(&scan_root)?;
    if !db_path.exists() {
        anyhow::bail!(
            "No cache found for '{}'. Run 'diskcopilot-cli scan {}' first.",
            scan_root.display(),
            scan_root.display()
        );
    }

    let insights = insights_content.map(|c| InsightsData { content: c });

    // Generate a random auth token for this session.
    // It's embedded in the served HTML and required for all mutating API calls.
    let auth_token = blake3::hash(
        &std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_le_bytes(),
    ).to_hex().to_string();

    let state = Arc::new(AppState {
        scan_root: scan_root.clone(),
        db_path,
        auth_token,
        insights: Mutex::new(insights),
    });

    // Only allow requests from our own origin (localhost on this port).
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::exact(
            format!("http://127.0.0.1:{}", port).parse().unwrap(),
        ));

    let app = Router::new()
        .route("/", get(index))
        .route("/api/info", get(api_info))
        .route("/api/tree", get(api_tree))
        .route("/api/large-files", get(api_large_files))
        .route("/api/dev-artifacts", get(api_dev_artifacts))
        .route("/api/old-files", get(api_old_files))
        .route("/api/summary", get(api_summary))
        .route("/api/insights", get(api_insights))
        .route("/api/insights", post(api_set_insights))
        .route("/api/trash", post(api_trash))
        .layer(cors)
        .with_state(state);

    let addr = format!("127.0.0.1:{}", port);
    let url = format!("http://{}", addr);

    eprintln!("\n\x1b[1;32m✓ diskcopilot server running\x1b[0m");
    eprintln!("  \x1b[1mURL:\x1b[0m     \x1b[36m{}\x1b[0m", url);
    eprintln!("  \x1b[1mPath:\x1b[0m    {}", scan_root.display());
    eprintln!("  \x1b[90mPress Ctrl+C to stop\x1b[0m\n");

    // Open browser
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(&url).spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
    }

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
