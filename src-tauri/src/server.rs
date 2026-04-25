use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, RwLock};
use tokio::fs as tfs;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::ServeDir;

use crate::config::{save_config, AppConfig};
use crate::indexer::FileIndex;
use crate::preview::{get_file_info, get_preview};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RwLock<AppConfig>>,
    pub index: Arc<FileIndex>,
    pub lan_port: Arc<RwLock<Option<u16>>>,
}

pub async fn start_server(state: AppState, port: u16, static_dir: std::path::PathBuf) {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let api = Router::new()
        .route("/api/health", get(health))
        .route("/api/status", get(status))
        .route("/api/search", get(search))
        .route("/api/rebuild", post(rebuild))
        .route("/api/file-info", get(file_info))
        .route("/api/preview", get(preview))
        .route("/api/file-stream", get(file_stream))
        .route("/api/open-file", post(open_file))
        .route("/api/open-in-explorer", post(open_in_explorer))
        .route("/api/open-terminal", post(open_terminal))
        .route("/api/lan-info", get(lan_info))
        .route("/api/config", get(get_config).post(post_config))
        .route("/api/volumes", get(volumes))
        .with_state(state)
        .layer(cors);

    let app = api.fallback_service(ServeDir::new(&static_dir).append_index_html_on_directories(true));

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    tracing::info!("Listening on http://{}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

// ── handlers ─────────────────────────────────────────────────────────────────

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true }))
}

async fn status(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(s.index.status()).unwrap())
}

async fn volumes() -> Json<serde_json::Value> {
    #[cfg(target_os = "windows")]
    {
        let vols: Vec<String> = (b'A'..=b'Z')
            .filter(|&d| Path::new(&format!("{}:\\", d as char)).exists())
            .map(|d| format!("{}:", d as char))
            .collect();
        return Json(serde_json::json!(vols));
    }
    #[cfg(not(target_os = "windows"))]
    Json(serde_json::json!(["/", "/home", "/tmp"]))
}

#[derive(Deserialize)]
struct SearchParams {
    query: Option<String>,
    #[serde(rename = "maxResults")]
    max_results: Option<usize>,
    #[serde(rename = "searchType")]
    search_type: Option<String>,
    #[serde(rename = "searchInPath")]
    search_in_path: Option<bool>,
}

async fn search(State(s): State<AppState>, Query(p): Query<SearchParams>) -> Json<serde_json::Value> {
    let results = s.index.search(
        p.query.as_deref().unwrap_or(""),
        p.max_results.unwrap_or(100),
        p.search_type.as_deref().unwrap_or("string"),
        p.search_in_path.unwrap_or(false),
    );
    Json(serde_json::to_value(results).unwrap())
}

async fn rebuild(State(s): State<AppState>, body: Option<Json<serde_json::Value>>) -> Json<serde_json::Value> {
    if let Some(Json(b)) = body {
        if let Some(dirs) = b.get("directories").and_then(|d| d.as_array()) {
            let dirs: Vec<String> = dirs.iter().filter_map(|v| v.as_str().map(String::from)).collect();
            let patterns = s.config.read().unwrap().exclude_patterns.clone();
            s.index.update_config(dirs, patterns);
        }
    }
    s.index.rebuild();
    Json(serde_json::to_value(s.index.status()).unwrap())
}

#[derive(Deserialize)]
struct PathParam {
    path: Option<String>,
}

async fn file_info(Query(p): Query<PathParam>) -> impl IntoResponse {
    match p.path.as_deref().and_then(|p| get_file_info(p)) {
        Some(info) => Json(serde_json::to_value(info).unwrap()).into_response(),
        None => (StatusCode::NOT_FOUND, Json(serde_json::json!({ "error": "not found" }))).into_response(),
    }
}

async fn preview(Query(p): Query<PathParam>) -> impl IntoResponse {
    match p.path {
        Some(path) => Json(serde_json::to_value(get_preview(&path)).unwrap()).into_response(),
        None => (StatusCode::BAD_REQUEST, Json(serde_json::json!({ "error": "missing path" }))).into_response(),
    }
}

async fn file_stream(Query(p): Query<PathParam>) -> impl IntoResponse {
    let path = match p.path {
        Some(p) => p,
        None => return (StatusCode::BAD_REQUEST, "missing path").into_response(),
    };
    if !Path::new(&path).exists() {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    let data = match tfs::read(&path).await {
        Ok(d) => d,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    let mime = mime_guess::from_path(&path).first_or_octet_stream();
    let filename = Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let disposition = if mime.type_() == "image" || mime.type_() == "audio" || mime.type_() == "video" || mime == mime_guess::mime::APPLICATION_PDF {
        format!("inline; filename=\"{}\"", filename)
    } else {
        format!("attachment; filename=\"{}\"", filename)
    };
    Response::builder()
        .header(header::CONTENT_TYPE, mime.to_string())
        .header(header::CONTENT_DISPOSITION, disposition)
        .header("Access-Control-Allow-Origin", "*")
        .body(Body::from(data))
        .unwrap()
        .into_response()
}

#[derive(Deserialize)]
struct OpenFileBody {
    path: Option<String>,
}

async fn open_file(State(s): State<AppState>, Json(body): Json<OpenFileBody>) -> Json<serde_json::Value> {
    let open_mode = s.config.read().unwrap().open_mode.clone();
    match open_mode.as_str() {
        "disabled" => Json(serde_json::json!({ "success": false })),
        "remote" => {
            let url = body.path.map(|p| format!("/api/file-stream?path={}", urlencoding::encode(&p)));
            Json(serde_json::json!({ "success": url.is_some(), "streamUrl": url }))
        }
        _ => {
            let ok = body.path.map(|p| open_native(&p)).unwrap_or(false);
            Json(serde_json::json!({ "success": ok }))
        }
    }
}

#[derive(Deserialize)]
struct ExplorerBody {
    path: Option<String>,
}

async fn open_in_explorer(State(s): State<AppState>, Json(body): Json<ExplorerBody>) -> Json<serde_json::Value> {
    if s.config.read().unwrap().open_mode != "local" {
        return Json(serde_json::json!({ "success": false }));
    }
    let ok = body.path.map(|p| open_in_explorer_native(&p)).unwrap_or(false);
    Json(serde_json::json!({ "success": ok }))
}

#[derive(Deserialize)]
struct TerminalBody {
    #[serde(rename = "workDir")]
    work_dir: Option<String>,
}

async fn open_terminal(State(s): State<AppState>, Json(body): Json<TerminalBody>) -> Json<serde_json::Value> {
    if s.config.read().unwrap().open_mode != "local" {
        return Json(serde_json::json!({ "success": false }));
    }
    let ok = body.work_dir.map(|d| open_terminal_native(&d)).unwrap_or(false);
    Json(serde_json::json!({ "success": ok }))
}

async fn lan_info(State(s): State<AppState>) -> Json<serde_json::Value> {
    let cfg = s.config.read().unwrap();
    let lan_enabled = cfg.lan_enabled;
    let lan_user = cfg.lan_user.clone();
    drop(cfg);
    let lan_port = s.lan_port.read().unwrap().unwrap_or(0);
    Json(serde_json::json!({
        "ip": get_lan_ip(),
        "port": lan_port,
        "lanEnabled": lan_enabled,
        "lanUser": lan_user,
    }))
}

async fn get_config(State(s): State<AppState>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(s.config.read().unwrap().clone()).unwrap())
}

async fn post_config(State(s): State<AppState>, Json(body): Json<serde_json::Value>) -> Json<serde_json::Value> {
    let mut cfg = s.config.write().unwrap();
    let mut dirs_changed = false;
    let mut exclude_changed = false;

    if let Some(dirs) = body.get("indexedDirectories").and_then(|v| v.as_array()) {
        let new_dirs: Vec<String> = dirs.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        if new_dirs != cfg.indexed_directories {
            cfg.indexed_directories = new_dirs;
            dirs_changed = true;
        }
    }
    if let Some(pats) = body.get("excludePatterns").and_then(|v| v.as_array()) {
        let new_pats: Vec<String> = pats.iter().filter_map(|v| v.as_str().map(String::from)).collect();
        if new_pats != cfg.exclude_patterns {
            cfg.exclude_patterns = new_pats;
            exclude_changed = true;
        }
    }
    if let Some(t) = body.get("theme").and_then(|v| v.as_str()) {
        cfg.theme = t.to_string();
    }
    if let Some(v) = body.get("lanEnabled").and_then(|v| v.as_bool()) {
        cfg.lan_enabled = v;
    }
    if let Some(v) = body.get("lanUser").and_then(|v| v.as_str()) {
        cfg.lan_user = v.to_string();
    }

    let _ = save_config(&cfg);
    let indexing = dirs_changed || exclude_changed;
    let cfg_clone = cfg.clone();
    drop(cfg);

    if indexing {
        let dirs = cfg_clone.indexed_directories.clone();
        let pats = cfg_clone.exclude_patterns.clone();
        s.index.update_config(dirs, pats);
        s.index.rebuild();
    }

    Json(serde_json::json!({ "success": true, "config": cfg_clone, "indexing": indexing }))
}

// ── native helpers ────────────────────────────────────────────────────────────

fn open_native(path: &str) -> bool {
    #[cfg(target_os = "windows")]
    { Command::new("cmd").args(["/c", "start", "", path]).spawn().is_ok() }
    #[cfg(target_os = "macos")]
    { Command::new("open").arg(path).spawn().is_ok() }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    { Command::new("xdg-open").arg(path).spawn().is_ok() }
}

fn open_in_explorer_native(path: &str) -> bool {
    #[cfg(target_os = "windows")]
    { Command::new("explorer").arg(format!("/select,{}", path)).spawn().is_ok() }
    #[cfg(target_os = "macos")]
    { Command::new("open").args(["-R", path]).spawn().is_ok() }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let dir = Path::new(path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        Command::new("xdg-open").arg(&dir).spawn().is_ok()
    }
}

fn open_terminal_native(work_dir: &str) -> bool {
    #[cfg(target_os = "windows")]
    { Command::new("cmd").args(["/c", "start", "cmd", "/k", &format!("cd /d \"{}\"", work_dir)]).spawn().is_ok() }
    #[cfg(target_os = "macos")]
    {
        let script = format!("tell app \"Terminal\" to do script \"cd '{}'\"", work_dir);
        Command::new("osascript").args(["-e", &script]).spawn().is_ok()
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    { Command::new("x-terminal-emulator").args(["-e", &format!("bash -c 'cd \"{}\" && $SHELL'", work_dir)]).spawn().is_ok() }
}

fn get_lan_ip() -> String {
    // Simple: iterate network interfaces via /proc or use a UDP trick
    use std::net::UdpSocket;
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|s| { s.connect("8.8.8.8:80")?; s.local_addr() })
        .map(|a| a.ip().to_string())
        .unwrap_or_else(|_| "unknown".to_string())
}
