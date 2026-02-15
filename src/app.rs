use anyhow::{Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        Path as AxumPath, State, WebSocketUpgrade,
    },
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse},
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use minijinja::{context, value::Value, Environment};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    net::{Ipv4Addr, Ipv6Addr},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::SystemTime,
};
use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc, Mutex},
};
use tower_http::cors::CorsLayer;

const TEMPLATE_NAME: &str = "main.html";
static TEMPLATE_ENV: OnceLock<Environment<'static>> = OnceLock::new();
const MERMAID_JS: &str = include_str!("../static/js/mermaid.min.js");
const MERMAID_ETAG: &str = concat!("\"", env!("CARGO_PKG_VERSION"), "\"");

type SharedMarkdownState = Arc<Mutex<MarkdownState>>;

fn template_env() -> &'static Environment<'static> {
    TEMPLATE_ENV.get_or_init(|| {
        let mut env = Environment::new();
        minijinja_embed::load_templates!(&mut env);
        env
    })
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
enum ClientMessage {
    Ping,
    RequestRefresh,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
enum ServerMessage {
    Reload,
    Pong,
}

use std::collections::HashMap;

pub(crate) fn scan_markdown_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut md_files = Vec::new();

    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() && is_markdown_file(&path) {
            md_files.push(path);
        }
    }

    md_files.sort();

    Ok(md_files)
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
        .unwrap_or(false)
}

struct TrackedFile {
    path: PathBuf,
    last_modified: SystemTime,
    html: String,
}

struct MarkdownState {
    base_dir: PathBuf,
    tracked_files: HashMap<String, TrackedFile>,
    is_directory_mode: bool,
    change_tx: broadcast::Sender<ServerMessage>,
}

impl MarkdownState {
    fn new(base_dir: PathBuf, file_paths: Vec<PathBuf>, is_directory_mode: bool) -> Result<Self> {
        let (change_tx, _) = broadcast::channel::<ServerMessage>(16);

        let mut tracked_files = HashMap::new();
        for file_path in file_paths {
            let metadata = fs::metadata(&file_path)?;
            let last_modified = metadata.modified()?;
            let content = fs::read_to_string(&file_path)?;
            let html = Self::markdown_to_html(&content)?;

            let filename = file_path.file_name().unwrap().to_string_lossy().to_string();

            tracked_files.insert(
                filename,
                TrackedFile {
                    path: file_path,
                    last_modified,
                    html,
                },
            );
        }

        Ok(MarkdownState {
            base_dir,
            tracked_files,
            is_directory_mode,
            change_tx,
        })
    }

    fn show_navigation(&self) -> bool {
        self.is_directory_mode
    }

    fn get_sorted_filenames(&self) -> Vec<String> {
        let mut filenames: Vec<_> = self.tracked_files.keys().cloned().collect();
        filenames.sort();
        filenames
    }

    fn refresh_file(&mut self, filename: &str) -> Result<()> {
        if let Some(tracked) = self.tracked_files.get_mut(filename) {
            let metadata = fs::metadata(&tracked.path)?;
            let current_modified = metadata.modified()?;

            if current_modified > tracked.last_modified {
                let content = fs::read_to_string(&tracked.path)?;
                tracked.html = Self::markdown_to_html(&content)?;
                tracked.last_modified = current_modified;
            }
        }

        Ok(())
    }

    fn add_tracked_file(&mut self, file_path: PathBuf) -> Result<()> {
        let filename = file_path.file_name().unwrap().to_string_lossy().to_string();

        if self.tracked_files.contains_key(&filename) {
            return Ok(());
        }

        let metadata = fs::metadata(&file_path)?;
        let content = fs::read_to_string(&file_path)?;

        self.tracked_files.insert(
            filename,
            TrackedFile {
                path: file_path,
                last_modified: metadata.modified()?,
                html: Self::markdown_to_html(&content)?,
            },
        );

        Ok(())
    }

    fn markdown_to_html(content: &str) -> Result<String> {
        let mut options = markdown::Options::gfm();
        options.compile.allow_dangerous_html = true;
        options.parse.constructs.frontmatter = true;

        let html_body = markdown::to_html_with_options(content, &options)
            .unwrap_or_else(|_| "Error parsing markdown".to_string());

        Ok(html_body)
    }
}

/// Handles a markdown file that may have been created or modified.
/// Refreshes tracked files or adds new files in directory mode, sending reload notifications.
async fn handle_markdown_file_change(path: &Path, state: &SharedMarkdownState) {
    if !is_markdown_file(path) {
        return;
    }

    let filename = path.file_name().and_then(|n| n.to_str()).map(String::from);
    let Some(filename) = filename else {
        return;
    };

    let mut state_guard = state.lock().await;

    // If file is already tracked, refresh its content
    if state_guard.tracked_files.contains_key(&filename) {
        if state_guard.refresh_file(&filename).is_ok() {
            let _ = state_guard.change_tx.send(ServerMessage::Reload);
        }
    } else if state_guard.is_directory_mode {
        // New file in directory mode - add and reload
        if state_guard.add_tracked_file(path.to_path_buf()).is_ok() {
            let _ = state_guard.change_tx.send(ServerMessage::Reload);
        }
    }
}

async fn handle_file_event(event: Event, state: &SharedMarkdownState) {
    match event.kind {
        notify::EventKind::Modify(notify::event::ModifyKind::Name(rename_mode)) => {
            use notify::event::RenameMode;
            match rename_mode {
                RenameMode::Both => {
                    // Linux/Windows: Both old and new paths provided in single event
                    if event.paths.len() == 2 {
                        let new_path = &event.paths[1];
                        handle_markdown_file_change(new_path, state).await;
                    }
                }
                RenameMode::From => {
                    // File being renamed away - ignore
                }
                RenameMode::To => {
                    // File renamed to this location
                    if let Some(path) = event.paths.first() {
                        handle_markdown_file_change(path, state).await;
                    }
                }
                RenameMode::Any => {
                    // macOS: Sends separate events for old and new paths
                    // Use file existence to distinguish old (doesn't exist) from new (exists)
                    if let Some(path) = event.paths.first() {
                        if path.exists() {
                            handle_markdown_file_change(path, state).await;
                        }
                    }
                }
                _ => {}
            }
        }
        _ => {
            for path in &event.paths {
                if is_markdown_file(path) {
                    match event.kind {
                        notify::EventKind::Create(_)
                        | notify::EventKind::Modify(notify::event::ModifyKind::Data(_)) => {
                            handle_markdown_file_change(path, state).await;
                        }
                        notify::EventKind::Remove(_) => {
                            // Don't remove files from tracking. Editors like neovim save by
                            // renaming the file to a backup, then creating a new one. If we
                            // removed the file here, HTTP requests during that window would
                            // see empty tracked_files and return 404.
                        }
                        _ => {}
                    }
                } else if path.is_file() && is_image_file(path.to_str().unwrap_or("")) {
                    match event.kind {
                        notify::EventKind::Modify(_)
                        | notify::EventKind::Create(_)
                        | notify::EventKind::Remove(_) => {
                            let state_guard = state.lock().await;
                            let _ = state_guard.change_tx.send(ServerMessage::Reload);
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

fn new_router(
    base_dir: PathBuf,
    tracked_files: Vec<PathBuf>,
    is_directory_mode: bool,
) -> Result<Router> {
    let base_dir = base_dir.canonicalize()?;

    let state = Arc::new(Mutex::new(MarkdownState::new(
        base_dir.clone(),
        tracked_files,
        is_directory_mode,
    )?));

    let watcher_state = state.clone();
    let (tx, mut rx) = mpsc::channel(100);

    let mut watcher = RecommendedWatcher::new(
        move |res: std::result::Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.blocking_send(event);
            }
        },
        Config::default(),
    )?;

    watcher.watch(&base_dir, RecursiveMode::NonRecursive)?;

    tokio::spawn(async move {
        let _watcher = watcher;
        while let Some(event) = rx.recv().await {
            handle_file_event(event, &watcher_state).await;
        }
    });

    let router = Router::new()
        .route("/", get(serve_html_root))
        .route("/ws", get(websocket_handler))
        .route("/mermaid.min.js", get(serve_mermaid_js))
        .route("/*filename", get(serve_file))
        .layer(CorsLayer::permissive())
        .with_state(state);

    Ok(router)
}

pub(crate) async fn serve_markdown(
    base_dir: PathBuf,
    tracked_files: Vec<PathBuf>,
    is_directory_mode: bool,
    hostname: impl AsRef<str>,
    port: u16,
    open: bool,
) -> Result<()> {
    let hostname = hostname.as_ref();

    let first_file = tracked_files.first().cloned();
    let router = new_router(base_dir.clone(), tracked_files, is_directory_mode)?;

    let listener = TcpListener::bind((hostname, port)).await?;

    let listen_addr = format_host(hostname, port);

    if is_directory_mode {
        println!("📁 Serving markdown files from: {}", base_dir.display());
    } else if let Some(file_path) = first_file {
        println!("📄 Serving markdown file: {}", file_path.display());
    }

    println!("🌐 Server running at: http://{listen_addr}");
    println!("⚡ Live reload enabled");
    println!("\nPress Ctrl+C to stop the server");

    if open {
        let browse_addr = format_host(&browsable_host(hostname), port);
        open_browser(&format!("http://{browse_addr}"))?;
    }

    axum::serve(listener, router).await?;

    Ok(())
}

/// Format the host address (hostname + port) for printing.
fn format_host(hostname: &str, port: u16) -> String {
    if hostname.parse::<Ipv6Addr>().is_ok() {
        format!("[{hostname}]:{port}")
    } else {
        format!("{hostname}:{port}")
    }
}

/// Map wildcard bind addresses to loopback so the browser gets a
/// reachable URL.
fn browsable_host(hostname: &str) -> String {
    if hostname
        .parse::<Ipv4Addr>()
        .ok()
        .is_some_and(|ip| ip.is_unspecified())
    {
        "127.0.0.1".into()
    } else if hostname
        .parse::<Ipv6Addr>()
        .ok()
        .is_some_and(|ip| ip.is_unspecified())
    {
        "::1".into()
    } else {
        hostname.into()
    }
}

/// Open a URL in the default browser using platform commands.
///
/// Fails immediately if the command cannot be spawned (e.g. not
/// installed). Exit status is monitored in a background thread
/// since opener commands may block until their handler process
/// returns.
fn open_browser(url: &str) -> Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "linux") {
        "xdg-open"
    } else {
        anyhow::bail!("--open is not supported on this platform");
    };

    let mut child = std::process::Command::new(program)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .with_context(|| format!("failed to run {program}"))?;

    std::thread::spawn(move || match child.wait() {
        Ok(status) if !status.success() => {
            eprintln!("{program} exited with {status}");
        }
        Err(e) => eprintln!("Failed waiting on {program}: {e}"),
        _ => {}
    });

    Ok(())
}

async fn serve_html_root(State(state): State<SharedMarkdownState>) -> impl IntoResponse {
    let mut state = state.lock().await;

    let filename = match state.get_sorted_filenames().into_iter().next() {
        Some(name) => name,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html("No files available to serve".to_string()),
            );
        }
    };

    let _ = state.refresh_file(&filename);

    render_markdown(&state, &filename).await
}

async fn serve_file(
    AxumPath(filename): AxumPath<String>,
    State(state): State<SharedMarkdownState>,
) -> axum::response::Response {
    if filename.ends_with(".md") || filename.ends_with(".markdown") {
        let mut state = state.lock().await;

        if !state.tracked_files.contains_key(&filename) {
            return (StatusCode::NOT_FOUND, Html("File not found".to_string())).into_response();
        }

        let _ = state.refresh_file(&filename);

        let (status, html) = render_markdown(&state, &filename).await;
        (status, html).into_response()
    } else if is_image_file(&filename) {
        serve_static_file_inner(filename, state).await
    } else {
        (StatusCode::NOT_FOUND, Html("File not found".to_string())).into_response()
    }
}

async fn render_markdown(state: &MarkdownState, current_file: &str) -> (StatusCode, Html<String>) {
    let env = template_env();
    let template = match env.get_template(TEMPLATE_NAME) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(format!("Template error: {e}")),
            );
        }
    };

    let (content, has_mermaid) = if let Some(tracked) = state.tracked_files.get(current_file) {
        let html = &tracked.html;
        let mermaid = html.contains(r#"class="language-mermaid""#);
        (Value::from_safe_string(html.clone()), mermaid)
    } else {
        return (StatusCode::NOT_FOUND, Html("File not found".to_string()));
    };

    let rendered = if state.show_navigation() {
        let filenames = state.get_sorted_filenames();
        let files: Vec<Value> = filenames
            .iter()
            .map(|name| {
                Value::from_object({
                    let mut map = std::collections::HashMap::new();
                    map.insert("name".to_string(), Value::from(name.clone()));
                    map
                })
            })
            .collect();

        match template.render(context! {
            content => content,
            mermaid_enabled => has_mermaid,
            show_navigation => true,
            files => files,
            current_file => current_file,
        }) {
            Ok(r) => r,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Html(format!("Rendering error: {e}")),
                );
            }
        }
    } else {
        match template.render(context! {
            content => content,
            mermaid_enabled => has_mermaid,
            show_navigation => false,
        }) {
            Ok(r) => r,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Html(format!("Rendering error: {e}")),
                );
            }
        }
    };

    (StatusCode::OK, Html(rendered))
}

async fn serve_mermaid_js(headers: HeaderMap) -> impl IntoResponse {
    if is_etag_match(&headers) {
        return mermaid_response(StatusCode::NOT_MODIFIED, None);
    }

    mermaid_response(StatusCode::OK, Some(MERMAID_JS))
}

fn is_etag_match(headers: &HeaderMap) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|etags| etags.split(',').any(|tag| tag.trim() == MERMAID_ETAG))
}

fn mermaid_response(status: StatusCode, body: Option<&'static str>) -> impl IntoResponse {
    // Use no-cache to force revalidation on each request. This ensures clients
    // get updated content when mdserve is rebuilt with a new Mermaid version,
    // while still benefiting from 304 responses via ETag matching.
    let headers = [
        (header::CONTENT_TYPE, "application/javascript"),
        (header::ETAG, MERMAID_ETAG),
        (header::CACHE_CONTROL, "public, no-cache"),
    ];

    match body {
        Some(content) => (status, headers, content).into_response(),
        None => (status, headers).into_response(),
    }
}

async fn serve_static_file_inner(
    filename: String,
    state: SharedMarkdownState,
) -> axum::response::Response {
    let state = state.lock().await;

    let full_path = state.base_dir.join(&filename);

    match full_path.canonicalize() {
        Ok(canonical_path) => {
            if !canonical_path.starts_with(&state.base_dir) {
                return (
                    StatusCode::FORBIDDEN,
                    [(header::CONTENT_TYPE, "text/plain")],
                    "Access denied".to_string(),
                )
                    .into_response();
            }

            match fs::read(&canonical_path) {
                Ok(contents) => {
                    let content_type = guess_image_content_type(&filename);
                    (
                        StatusCode::OK,
                        [(header::CONTENT_TYPE, content_type.as_str())],
                        contents,
                    )
                        .into_response()
                }
                Err(_) => (
                    StatusCode::NOT_FOUND,
                    [(header::CONTENT_TYPE, "text/plain")],
                    "File not found".to_string(),
                )
                    .into_response(),
            }
        }
        Err(_) => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain")],
            "File not found".to_string(),
        )
            .into_response(),
    }
}

fn is_image_file(file_path: &str) -> bool {
    let extension = std::path::Path::new(file_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");

    matches!(
        extension.to_lowercase().as_str(),
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "bmp" | "ico"
    )
}

fn guess_image_content_type(file_path: &str) -> String {
    let extension = std::path::Path::new(file_path)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("");

    match extension.to_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "ico" => "image/x-icon",
        _ => "application/octet-stream",
    }
    .to_string()
}

async fn websocket_handler(
    ws: WebSocketUpgrade,
    State(state): State<SharedMarkdownState>,
) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_websocket(socket, state))
}

async fn handle_websocket(socket: WebSocket, state: SharedMarkdownState) {
    let (mut sender, mut receiver) = socket.split();

    let mut change_rx = {
        let state = state.lock().await;
        state.change_tx.subscribe()
    };

    let recv_task = tokio::spawn(async move {
        while let Some(msg) = receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                        match client_msg {
                            ClientMessage::Ping | ClientMessage::RequestRefresh => {}
                        }
                    }
                }
                Ok(Message::Close(_)) => break,
                _ => {}
            }
        }
    });

    let send_task = tokio::spawn(async move {
        while let Ok(reload_msg) = change_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&reload_msg) {
                if sender.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        }
    });

    tokio::select! {
        _ = recv_task => {},
        _ = send_task => {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_is_markdown_file() {
        assert!(is_markdown_file(Path::new("test.md")));
        assert!(is_markdown_file(Path::new("/path/to/file.md")));

        assert!(is_markdown_file(Path::new("test.markdown")));
        assert!(is_markdown_file(Path::new("/path/to/file.markdown")));

        assert!(is_markdown_file(Path::new("test.MD")));
        assert!(is_markdown_file(Path::new("test.Md")));
        assert!(is_markdown_file(Path::new("test.MARKDOWN")));
        assert!(is_markdown_file(Path::new("test.MarkDown")));

        assert!(!is_markdown_file(Path::new("test.txt")));
        assert!(!is_markdown_file(Path::new("test.rs")));
        assert!(!is_markdown_file(Path::new("test.html")));
        assert!(!is_markdown_file(Path::new("test")));
        assert!(!is_markdown_file(Path::new("README")));
    }

    #[test]
    fn test_is_image_file() {
        assert!(is_image_file("test.png"));
        assert!(is_image_file("test.jpg"));
        assert!(is_image_file("test.jpeg"));
        assert!(is_image_file("test.gif"));
        assert!(is_image_file("test.svg"));
        assert!(is_image_file("test.webp"));
        assert!(is_image_file("test.bmp"));
        assert!(is_image_file("test.ico"));

        assert!(is_image_file("test.PNG"));
        assert!(is_image_file("test.JPG"));
        assert!(is_image_file("test.JPEG"));

        assert!(is_image_file("/path/to/image.png"));
        assert!(is_image_file("./images/photo.jpg"));

        assert!(!is_image_file("test.txt"));
        assert!(!is_image_file("test.md"));
        assert!(!is_image_file("test.rs"));
        assert!(!is_image_file("test"));
    }

    #[test]
    fn test_guess_image_content_type() {
        assert_eq!(guess_image_content_type("test.png"), "image/png");
        assert_eq!(guess_image_content_type("test.jpg"), "image/jpeg");
        assert_eq!(guess_image_content_type("test.jpeg"), "image/jpeg");
        assert_eq!(guess_image_content_type("test.gif"), "image/gif");
        assert_eq!(guess_image_content_type("test.svg"), "image/svg+xml");
        assert_eq!(guess_image_content_type("test.webp"), "image/webp");
        assert_eq!(guess_image_content_type("test.bmp"), "image/bmp");
        assert_eq!(guess_image_content_type("test.ico"), "image/x-icon");

        assert_eq!(guess_image_content_type("test.PNG"), "image/png");
        assert_eq!(guess_image_content_type("test.JPG"), "image/jpeg");

        assert_eq!(
            guess_image_content_type("test.xyz"),
            "application/octet-stream"
        );
        assert_eq!(guess_image_content_type("test"), "application/octet-stream");
    }

    #[test]
    fn test_scan_markdown_files_empty_directory() {
        let temp_dir = tempdir().expect("Failed to create temp dir");

        let result = scan_markdown_files(temp_dir.path()).expect("Failed to scan");
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_scan_markdown_files_with_markdown_files() {
        let temp_dir = tempdir().expect("Failed to create temp dir");

        fs::write(temp_dir.path().join("test1.md"), "# Test 1").expect("Failed to write");
        fs::write(temp_dir.path().join("test2.markdown"), "# Test 2").expect("Failed to write");
        fs::write(temp_dir.path().join("test3.md"), "# Test 3").expect("Failed to write");

        fs::write(temp_dir.path().join("test.txt"), "text").expect("Failed to write");
        fs::write(temp_dir.path().join("README"), "readme").expect("Failed to write");

        let result = scan_markdown_files(temp_dir.path()).expect("Failed to scan");

        assert_eq!(result.len(), 3);

        let filenames: Vec<_> = result
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap())
            .collect();
        assert_eq!(filenames, vec!["test1.md", "test2.markdown", "test3.md"]);
    }

    #[test]
    fn test_scan_markdown_files_ignores_subdirectories() {
        let temp_dir = tempdir().expect("Failed to create temp dir");

        fs::write(temp_dir.path().join("root.md"), "# Root").expect("Failed to write");

        let sub_dir = temp_dir.path().join("subdir");
        fs::create_dir(&sub_dir).expect("Failed to create subdir");
        fs::write(sub_dir.join("nested.md"), "# Nested").expect("Failed to write");

        let result = scan_markdown_files(temp_dir.path()).expect("Failed to scan");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].file_name().unwrap().to_str().unwrap(), "root.md");
    }

    #[test]
    fn test_scan_markdown_files_case_insensitive() {
        let temp_dir = tempdir().expect("Failed to create temp dir");

        fs::write(temp_dir.path().join("test1.md"), "# Test 1").expect("Failed to write");
        fs::write(temp_dir.path().join("test2.MD"), "# Test 2").expect("Failed to write");
        fs::write(temp_dir.path().join("test3.Md"), "# Test 3").expect("Failed to write");
        fs::write(temp_dir.path().join("test4.MARKDOWN"), "# Test 4").expect("Failed to write");

        let result = scan_markdown_files(temp_dir.path()).expect("Failed to scan");

        assert_eq!(result.len(), 4);
    }

    #[test]
    fn test_format_host() {
        assert_eq!(format_host("127.0.0.1", 3000), "127.0.0.1:3000");
        assert_eq!(format_host("192.168.1.1", 8080), "192.168.1.1:8080");

        assert_eq!(format_host("localhost", 3000), "localhost:3000");
        assert_eq!(format_host("example.com", 80), "example.com:80");

        assert_eq!(format_host("::1", 3000), "[::1]:3000");
        assert_eq!(format_host("2001:db8::1", 8080), "[2001:db8::1]:8080");
    }

    #[test]
    fn test_browsable_host() {
        assert_eq!(browsable_host("0.0.0.0"), "127.0.0.1");
        assert_eq!(browsable_host("::"), "::1");
        assert_eq!(browsable_host("127.0.0.1"), "127.0.0.1");
        assert_eq!(browsable_host("::1"), "::1");
        assert_eq!(browsable_host("192.168.1.1"), "192.168.1.1");
        assert_eq!(browsable_host("localhost"), "localhost");
        assert_eq!(browsable_host("example.com"), "example.com");
    }

    use axum_test::TestServer;
    use std::time::Duration;
    use tempfile::{Builder, NamedTempFile, TempDir};

    const FILE_WATCH_DELAY_MS: u64 = 100;
    const WEBSOCKET_TIMEOUT_SECS: u64 = 5;

    const TEST_FILE_1_CONTENT: &str = "# Test 1\n\nContent of test1";
    const TEST_FILE_2_CONTENT: &str = "# Test 2\n\nContent of test2";
    const TEST_FILE_3_CONTENT: &str = "# Test 3\n\nContent of test3";
    const YAML_FRONTMATTER_CONTENT: &str =
        "---\ntitle: Test Post\nauthor: Name\n---\n\n# Test Post\n";
    const TOML_FRONTMATTER_CONTENT: &str = "+++\ntitle = \"Test Post\"\n+++\n\n# Test Post\n";

    fn create_test_server_impl(content: &str, use_http: bool) -> (TestServer, NamedTempFile) {
        let temp_file = Builder::new()
            .suffix(".md")
            .tempfile()
            .expect("Failed to create temp file");
        fs::write(&temp_file, content).expect("Failed to write temp file");

        let canonical_path = temp_file
            .path()
            .canonicalize()
            .unwrap_or_else(|_| temp_file.path().to_path_buf());

        let base_dir = canonical_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        let tracked_files = vec![canonical_path];
        let is_directory_mode = false;

        let router = new_router(base_dir, tracked_files, is_directory_mode)
            .expect("Failed to create router");

        let server = if use_http {
            TestServer::builder()
                .http_transport()
                .build(router)
                .expect("Failed to create test server")
        } else {
            TestServer::new(router).expect("Failed to create test server")
        };

        (server, temp_file)
    }

    async fn create_test_server(content: &str) -> (TestServer, NamedTempFile) {
        create_test_server_impl(content, false)
    }

    async fn create_test_server_with_http(content: &str) -> (TestServer, NamedTempFile) {
        create_test_server_impl(content, true)
    }

    fn create_directory_server_impl(use_http: bool) -> (TestServer, TempDir) {
        let temp_dir = tempdir().expect("Failed to create temp dir");

        fs::write(temp_dir.path().join("test1.md"), TEST_FILE_1_CONTENT)
            .expect("Failed to write test1.md");
        fs::write(temp_dir.path().join("test2.markdown"), TEST_FILE_2_CONTENT)
            .expect("Failed to write test2.markdown");
        fs::write(temp_dir.path().join("test3.md"), TEST_FILE_3_CONTENT)
            .expect("Failed to write test3.md");

        let base_dir = temp_dir.path().to_path_buf();
        let tracked_files = scan_markdown_files(&base_dir).expect("Failed to scan markdown files");
        let is_directory_mode = true;

        let router = new_router(base_dir, tracked_files, is_directory_mode)
            .expect("Failed to create router");

        let server = if use_http {
            TestServer::builder()
                .http_transport()
                .build(router)
                .expect("Failed to create test server")
        } else {
            TestServer::new(router).expect("Failed to create test server")
        };

        (server, temp_dir)
    }

    async fn create_directory_server() -> (TestServer, TempDir) {
        create_directory_server_impl(false)
    }

    async fn create_directory_server_with_http() -> (TestServer, TempDir) {
        create_directory_server_impl(true)
    }

    #[tokio::test]
    async fn test_server_starts_and_serves_basic_markdown() {
        let (server, _temp_file) =
            create_test_server("# Hello World\n\nThis is **bold** text.").await;

        let response = server.get("/").await;

        assert_eq!(response.status_code(), 200);
        let body = response.text();

        assert!(body.contains("<h1>Hello World</h1>"));
        assert!(body.contains("<strong>bold</strong>"));
        assert!(body.contains("theme-toggle"));
        assert!(body.contains("openThemeModal"));
        assert!(body.contains("--bg-color"));
        assert!(body.contains("data-theme=\"dark\""));
    }

    #[tokio::test]
    async fn test_websocket_connection() {
        let (server, _temp_file) = create_test_server_with_http("# WebSocket Test").await;

        let response = server.get_websocket("/ws").await;
        response.assert_status_switching_protocols();
    }

    #[tokio::test]
    async fn test_file_modification_updates_via_websocket() {
        let (server, temp_file) = create_test_server_with_http("# Original Content").await;

        let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

        fs::write(&temp_file, "# Modified Content").expect("Failed to modify file");

        tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

        let update_result = tokio::time::timeout(
            Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
            websocket.receive_json::<ServerMessage>(),
        )
        .await;

        match update_result {
            Ok(update_message) => {
                if let ServerMessage::Reload = update_message {
                    // Success
                } else {
                    panic!("Expected Reload message after file modification");
                }
            }
            Err(_) => {
                panic!("Timeout waiting for WebSocket update after file modification");
            }
        }
    }

    #[tokio::test]
    async fn test_server_handles_gfm_features() {
        let markdown_content = r#"# GFM Test

## Table
| Name | Age |
|------|-----|
| John | 30  |
| Jane | 25  |

## Strikethrough
~~deleted text~~

## Code block
```rust
fn main() {
    println!("Hello!");
}
```
"#;

        let (server, _temp_file) = create_test_server(markdown_content).await;

        let response = server.get("/").await;

        assert_eq!(response.status_code(), 200);
        let body = response.text();

        assert!(body.contains("<table>"));
        assert!(body.contains("<th>Name</th>"));
        assert!(body.contains("<td>John</td>"));
        assert!(body.contains("<del>deleted text</del>"));
        assert!(body.contains("<pre>"));
        assert!(body.contains("fn main()"));
    }

    #[tokio::test]
    async fn test_404_for_unknown_routes() {
        let (server, _temp_file) = create_test_server("# 404 Test").await;

        let response = server.get("/unknown-route").await;

        assert_eq!(response.status_code(), 404);
    }

    #[tokio::test]
    async fn test_image_serving() {
        let temp_dir = tempdir().expect("Failed to create temp dir");

        let md_content =
            "# Test with Image\n\n![Test Image](test.png)\n\nThis markdown references an image.";
        let md_path = temp_dir.path().join("test.md");
        fs::write(&md_path, md_content).expect("Failed to write markdown file");

        let png_data = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0xF8, 0x0F, 0x00, 0x00, 0x01, 0x00, 0x01, 0x5C, 0xDD, 0x8D, 0xB4, 0x00,
            0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ];
        let img_path = temp_dir.path().join("test.png");
        fs::write(&img_path, png_data).expect("Failed to write image file");

        let base_dir = temp_dir.path().to_path_buf();
        let tracked_files = vec![md_path];
        let is_directory_mode = false;
        let router = new_router(base_dir, tracked_files, is_directory_mode)
            .expect("Failed to create router");
        let server = TestServer::new(router).expect("Failed to create test server");

        let response = server.get("/").await;
        assert_eq!(response.status_code(), 200);
        let body = response.text();
        assert!(body.contains("<img src=\"test.png\" alt=\"Test Image\""));

        let img_response = server.get("/test.png").await;
        assert_eq!(img_response.status_code(), 200);
        assert_eq!(img_response.header("content-type"), "image/png");
        assert!(!img_response.as_bytes().is_empty());
    }

    #[tokio::test]
    async fn test_non_image_files_not_served() {
        let temp_dir = tempdir().expect("Failed to create temp dir");

        let md_content = "# Test";
        let md_path = temp_dir.path().join("test.md");
        fs::write(&md_path, md_content).expect("Failed to write markdown file");

        let txt_path = temp_dir.path().join("secret.txt");
        fs::write(&txt_path, "secret content").expect("Failed to write txt file");

        let base_dir = temp_dir.path().to_path_buf();
        let tracked_files = vec![md_path];
        let is_directory_mode = false;
        let router = new_router(base_dir, tracked_files, is_directory_mode)
            .expect("Failed to create router");
        let server = TestServer::new(router).expect("Failed to create test server");

        let response = server.get("/secret.txt").await;
        assert_eq!(response.status_code(), 404);
    }

    #[tokio::test]
    async fn test_html_tags_in_markdown_are_rendered() {
        let markdown_content = r#"# HTML Test

This markdown contains HTML tags:

<div class="highlight">
    <p>This should be rendered as HTML, not escaped</p>
    <span style="color: red;">Red text</span>
</div>

Regular **markdown** still works.
"#;

        let (server, _temp_file) = create_test_server(markdown_content).await;

        let response = server.get("/").await;

        assert_eq!(response.status_code(), 200);
        let body = response.text();

        assert!(body.contains(r#"<div class="highlight">"#));
        assert!(body.contains(r#"<span style="color: red;">"#));
        assert!(body.contains("<p>This should be rendered as HTML, not escaped</p>"));
        assert!(!body.contains("&lt;div"));
        assert!(!body.contains("&gt;"));
        assert!(body.contains("<strong>markdown</strong>"));
    }

    #[tokio::test]
    async fn test_mermaid_diagram_detection_and_script_injection() {
        let markdown_content = r#"# Mermaid Test

Regular content here.

```mermaid
graph TD
    A[Start] --> B{Decision}
    B -->|Yes| C[End]
    B -->|No| D[Continue]
```

More regular content.

```javascript
// This is a regular code block, not mermaid
console.log("Hello World");
```
"#;

        let (server, _temp_file) = create_test_server(markdown_content).await;

        let response = server.get("/").await;

        assert_eq!(response.status_code(), 200);
        let body = response.text();

        assert!(body.contains(r#"class="language-mermaid""#));
        assert!(body.contains("graph TD"));

        let has_raw_content = body.contains("A[Start] --> B{Decision}");
        let has_encoded_content = body.contains("A[Start] --&gt; B{Decision}");
        assert!(
            has_raw_content || has_encoded_content,
            "Expected mermaid content not found in body"
        );

        assert!(body.contains(r#"<script src="/mermaid.min.js"></script>"#));
        assert!(body.contains("function initMermaid()"));
        assert!(body.contains("function transformMermaidCodeBlocks()"));
        assert!(body.contains("function getMermaidTheme()"));
        assert!(body.contains(r#"class="language-javascript""#));
        assert!(body.contains("console.log"));
    }

    #[tokio::test]
    async fn test_no_mermaid_script_injection_without_mermaid_blocks() {
        let markdown_content = r#"# No Mermaid Test

This content has no mermaid diagrams.

```javascript
console.log("Hello World");
```

```bash
echo "Regular code block"
```

Just regular markdown content.
"#;

        let (server, _temp_file) = create_test_server(markdown_content).await;

        let response = server.get("/").await;

        assert_eq!(response.status_code(), 200);
        let body = response.text();

        assert!(!body.contains(r#"<script src="https://cdn.jsdelivr.net/npm/mermaid@11.12.0/dist/mermaid.min.js"></script>"#));
        assert!(body.contains("function initMermaid()"));
        assert!(body.contains(r#"class="language-javascript""#));
        assert!(body.contains(r#"class="language-bash""#));
    }

    #[tokio::test]
    async fn test_multiple_mermaid_diagrams() {
        let markdown_content = r#"# Multiple Mermaid Diagrams

## Flowchart
```mermaid
graph LR
    A --> B
```

## Sequence Diagram
```mermaid
sequenceDiagram
    Alice->>Bob: Hello
    Bob-->>Alice: Hi
```

## Class Diagram
```mermaid
classDiagram
    Animal <|-- Duck
```
"#;

        let (server, _temp_file) = create_test_server(markdown_content).await;

        let response = server.get("/").await;

        assert_eq!(response.status_code(), 200);
        let body = response.text();

        let mermaid_occurrences = body.matches(r#"class="language-mermaid""#).count();
        assert_eq!(mermaid_occurrences, 3);

        assert!(body.contains("graph LR"));
        assert!(body.contains("sequenceDiagram"));
        assert!(body.contains("classDiagram"));

        assert!(body.contains("A --&gt; B") || body.contains("A --> B"));
        assert!(body.contains("Alice-&gt;&gt;Bob") || body.contains("Alice->>Bob"));
        assert!(body.contains("Animal &lt;|-- Duck") || body.contains("Animal <|-- Duck"));

        let script_occurrences = body
            .matches(r#"<script src="/mermaid.min.js"></script>"#)
            .count();
        assert_eq!(script_occurrences, 1);
    }

    #[tokio::test]
    async fn test_mermaid_js_etag_caching() {
        let (server, _temp_file) = create_test_server("# Test").await;

        let response = server.get("/mermaid.min.js").await;
        assert_eq!(response.status_code(), 200);

        let etag = response.header("etag");
        assert!(!etag.is_empty(), "ETag header should be present");

        let cache_control = response.header("cache-control");
        let cache_control_str = cache_control.to_str().unwrap();
        assert!(cache_control_str.contains("public"));
        assert!(cache_control_str.contains("no-cache"));

        let content_type = response.header("content-type");
        assert_eq!(content_type, "application/javascript");

        assert!(!response.as_bytes().is_empty());

        let response_304 = server
            .get("/mermaid.min.js")
            .add_header(
                axum::http::header::IF_NONE_MATCH,
                axum::http::HeaderValue::from_str(etag.to_str().unwrap()).unwrap(),
            )
            .await;

        assert_eq!(response_304.status_code(), 304);
        assert_eq!(response_304.header("etag"), etag);
        assert!(response_304.as_bytes().is_empty());

        let response_200 = server
            .get("/mermaid.min.js")
            .add_header(
                axum::http::header::IF_NONE_MATCH,
                axum::http::HeaderValue::from_static("\"different-etag\""),
            )
            .await;

        assert_eq!(response_200.status_code(), 200);
        assert!(!response_200.as_bytes().is_empty());
    }

    #[tokio::test]
    async fn test_directory_mode_serves_multiple_files() {
        let (server, _temp_dir) = create_directory_server().await;

        let response1 = server.get("/test1.md").await;
        assert_eq!(response1.status_code(), 200);
        let body1 = response1.text();
        assert!(body1.contains("<h1>Test 1</h1>"));
        assert!(body1.contains("Content of test1"));

        let response2 = server.get("/test2.markdown").await;
        assert_eq!(response2.status_code(), 200);
        let body2 = response2.text();
        assert!(body2.contains("<h1>Test 2</h1>"));
        assert!(body2.contains("Content of test2"));

        let response3 = server.get("/test3.md").await;
        assert_eq!(response3.status_code(), 200);
        let body3 = response3.text();
        assert!(body3.contains("<h1>Test 3</h1>"));
        assert!(body3.contains("Content of test3"));
    }

    #[tokio::test]
    async fn test_directory_mode_file_not_found() {
        let (server, _temp_dir) = create_directory_server().await;

        let response = server.get("/nonexistent.md").await;
        assert_eq!(response.status_code(), 404);
    }

    #[tokio::test]
    async fn test_directory_mode_has_navigation_sidebar() {
        let (server, _temp_dir) = create_directory_server().await;

        let response = server.get("/test1.md").await;
        assert_eq!(response.status_code(), 200);
        let body = response.text();

        assert!(body.contains(r#"<nav class="sidebar">"#));
        assert!(body.contains(r#"<ul class="file-list">"#));
        assert!(body.contains("test1.md"));
        assert!(body.contains("test2.markdown"));
        assert!(body.contains("test3.md"));
    }

    #[tokio::test]
    async fn test_single_file_mode_no_navigation_sidebar() {
        let (server, _temp_file) = create_test_server("# Single File Test").await;

        let response = server.get("/").await;
        assert_eq!(response.status_code(), 200);
        let body = response.text();

        assert!(!body.contains(r#"<nav class="sidebar">"#));
        assert!(!body.contains("<h3>Files</h3>"));
        assert!(!body.contains(r#"<ul class="file-list">"#));
    }

    #[tokio::test]
    async fn test_directory_mode_active_file_highlighting() {
        let (server, _temp_dir) = create_directory_server().await;

        let response1 = server.get("/test1.md").await;
        assert_eq!(response1.status_code(), 200);
        let body1 = response1.text();

        assert!(
            body1.contains(r#"href="/test1.md" class="active""#),
            "test1.md link should have href and class on same line"
        );

        let active_link_count = body1.matches(r#"class="active""#).count();
        assert_eq!(active_link_count, 1, "Should have exactly one active link");

        let response2 = server.get("/test2.markdown").await;
        assert_eq!(response2.status_code(), 200);
        let body2 = response2.text();

        assert!(
            body2.contains(r#"href="/test2.markdown" class="active""#),
            "test2.markdown link should have href and class on same line"
        );
    }

    #[tokio::test]
    async fn test_directory_mode_file_order() {
        let (server, _temp_dir) = create_directory_server().await;

        let response = server.get("/test1.md").await;
        assert_eq!(response.status_code(), 200);
        let body = response.text();

        let test1_pos = body.find("test1.md").expect("test1.md not found");
        let test2_pos = body
            .find("test2.markdown")
            .expect("test2.markdown not found");
        let test3_pos = body.find("test3.md").expect("test3.md not found");

        assert!(
            test1_pos < test2_pos,
            "test1.md should appear before test2.markdown"
        );
        assert!(
            test2_pos < test3_pos,
            "test2.markdown should appear before test3.md"
        );
    }

    #[tokio::test]
    async fn test_directory_mode_websocket_file_modification() {
        let (server, temp_dir) = create_directory_server_with_http().await;

        let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

        let test_file = temp_dir.path().join("test1.md");
        fs::write(&test_file, "# Modified Test 1\n\nContent has changed")
            .expect("Failed to modify file");

        tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

        let update_result = tokio::time::timeout(
            Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
            websocket.receive_json::<ServerMessage>(),
        )
        .await;

        match update_result {
            Ok(update_message) => {
                if let ServerMessage::Reload = update_message {
                    // Success
                } else {
                    panic!("Expected Reload message after file modification");
                }
            }
            Err(_) => {
                panic!("Timeout waiting for WebSocket update after file modification");
            }
        }
    }

    #[tokio::test]
    async fn test_directory_mode_new_file_triggers_reload() {
        let (server, temp_dir) = create_directory_server_with_http().await;

        let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

        let new_file = temp_dir.path().join("test4.md");
        fs::write(&new_file, "# Test 4\n\nThis is a new file").expect("Failed to create new file");

        tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

        let update_result = tokio::time::timeout(
            Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
            websocket.receive_json::<ServerMessage>(),
        )
        .await;

        match update_result {
            Ok(update_message) => {
                if let ServerMessage::Reload = update_message {
                    // Success
                } else {
                    panic!("Expected Reload message after new file creation");
                }
            }
            Err(_) => {
                panic!("Timeout waiting for WebSocket update after new file creation");
            }
        }

        let response = server.get("/test1.md").await;
        assert_eq!(response.status_code(), 200);
        let body = response.text();

        assert!(
            body.contains("test4.md"),
            "New file should appear in navigation"
        );

        let new_file_response = server.get("/test4.md").await;
        assert_eq!(new_file_response.status_code(), 200);
        let new_file_body = new_file_response.text();
        assert!(new_file_body.contains("<h1>Test 4</h1>"));
        assert!(new_file_body.contains("This is a new file"));
    }

    #[tokio::test]
    async fn test_editor_save_simulation_single_file_mode() {
        let (server, temp_file) =
            create_test_server_with_http("# Original\n\nOriginal content").await;

        let file_path = temp_file.path().to_path_buf();
        let backup_path = file_path.with_extension("md~");

        let initial_response = server.get("/").await;
        assert_eq!(initial_response.status_code(), 200);
        assert!(initial_response.text().contains("Original content"));

        fs::rename(&file_path, &backup_path).expect("Failed to rename to backup");

        tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

        let during_save_response = server.get("/").await;
        assert_eq!(
            during_save_response.status_code(),
            200,
            "File should not return 404 during editor save"
        );

        fs::write(&file_path, "# Updated\n\nUpdated content").expect("Failed to write new file");

        tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

        let final_response = server.get("/").await;
        assert_eq!(final_response.status_code(), 200);
        let final_body = final_response.text();
        assert!(
            final_body.contains("Updated content"),
            "Should serve updated content after save"
        );
        assert!(
            !final_body.contains("Original content"),
            "Should not serve old content"
        );

        let _ = fs::remove_file(&backup_path);
    }

    #[tokio::test]
    async fn test_editor_save_simulation_directory_mode() {
        let (server, temp_dir) = create_directory_server_with_http().await;

        let file_path = temp_dir.path().join("test1.md");
        let backup_path = temp_dir.path().join("test1.md~");

        let initial_response = server.get("/test1.md").await;
        assert_eq!(initial_response.status_code(), 200);
        assert!(initial_response.text().contains("Content of test1"));

        fs::rename(&file_path, &backup_path).expect("Failed to rename to backup");

        tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

        let during_save_response = server.get("/test1.md").await;
        assert_eq!(
            during_save_response.status_code(),
            200,
            "File should not return 404 during editor save in directory mode"
        );

        fs::write(&file_path, "# Test 1 Updated\n\nUpdated content")
            .expect("Failed to write new file");

        tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

        let final_response = server.get("/test1.md").await;
        assert_eq!(final_response.status_code(), 200);
        let final_body = final_response.text();
        assert!(
            final_body.contains("Updated content"),
            "Should serve updated content after save"
        );

        let _ = fs::remove_file(&backup_path);
    }

    #[tokio::test]
    async fn test_no_404_during_editor_save_sequence() {
        let (server, temp_dir) = create_directory_server_with_http().await;
        let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

        let file_path = temp_dir.path().join("test1.md");
        let backup_path = temp_dir.path().join("test1.md~");

        fs::rename(&file_path, &backup_path).expect("Failed to rename to backup");
        tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

        let response_after_rename = server.get("/test1.md").await;
        assert_eq!(
            response_after_rename.status_code(),
            200,
            "Should not get 404 after rename to backup"
        );

        fs::write(&file_path, "# Test 1 Updated\n\nNew content").expect("Failed to write new file");
        tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

        let response_after_create = server.get("/test1.md").await;
        assert_eq!(
            response_after_create.status_code(),
            200,
            "Should successfully serve after new file created"
        );
        assert!(response_after_create.text().contains("New content"));

        let update_result = tokio::time::timeout(
            Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
            websocket.receive_json::<ServerMessage>(),
        )
        .await;

        assert!(update_result.is_ok(), "Should receive reload after save");

        let _ = fs::remove_file(&backup_path);
    }

    #[tokio::test]
    async fn test_yaml_frontmatter_is_stripped() {
        let (server, _temp_file) = create_test_server(YAML_FRONTMATTER_CONTENT).await;

        let response = server.get("/").await;

        assert_eq!(response.status_code(), 200);
        let body = response.text();

        assert!(!body.contains("title: Test Post"));
        assert!(!body.contains("author: Name"));
        assert!(body.contains("<h1>Test Post</h1>"));
    }

    #[tokio::test]
    async fn test_toml_frontmatter_is_stripped() {
        let (server, _temp_file) = create_test_server(TOML_FRONTMATTER_CONTENT).await;

        let response = server.get("/").await;

        assert_eq!(response.status_code(), 200);
        let body = response.text();

        assert!(!body.contains("title = \"Test Post\""));
        assert!(body.contains("<h1>Test Post</h1>"));
    }

    #[tokio::test]
    async fn test_temp_file_rename_triggers_reload_single_file_mode() {
        let (server, temp_file) =
            create_test_server_with_http("# Original\n\nOriginal content").await;

        let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

        let file_path = temp_file.path().to_path_buf();
        let temp_write_path = file_path.with_extension("md.tmp.12345");

        let initial_response = server.get("/").await;
        assert_eq!(initial_response.status_code(), 200);
        assert!(
            initial_response.text().contains("Original content"),
            "File should be tracked and serving content before edit"
        );

        fs::write(
            &temp_write_path,
            "# Updated\n\nUpdated content via temp file",
        )
        .expect("Failed to write temp file");

        tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

        fs::rename(&temp_write_path, &file_path).expect("Failed to rename temp file");

        tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

        let update_result = tokio::time::timeout(
            Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
            websocket.receive_json::<ServerMessage>(),
        )
        .await;

        match update_result {
            Ok(update_message) => {
                if let ServerMessage::Reload = update_message {
                    // Success
                } else {
                    panic!("Expected Reload message after temp file rename");
                }
            }
            Err(_) => {
                panic!("Timeout waiting for WebSocket update after temp file rename");
            }
        }

        let final_response = server.get("/").await;
        assert_eq!(final_response.status_code(), 200);
        let final_body = final_response.text();
        assert!(
            final_body.contains("Updated content via temp file"),
            "Should serve updated content after temp file rename"
        );
        assert!(
            !final_body.contains("Original content"),
            "Should not serve old content"
        );
    }

    #[tokio::test]
    async fn test_temp_file_rename_triggers_reload_directory_mode() {
        let (server, temp_dir) = create_directory_server_with_http().await;

        let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

        let file_path = temp_dir.path().join("test1.md");
        let temp_write_path = temp_dir.path().join("test1.md.tmp.67890");

        let initial_response = server.get("/test1.md").await;
        assert_eq!(initial_response.status_code(), 200);
        assert!(
            initial_response.text().contains("Content of test1"),
            "File should be tracked and serving content before edit"
        );

        fs::write(
            &temp_write_path,
            "# Test 1 Updated\n\nUpdated via temp file rename",
        )
        .expect("Failed to write temp file");

        tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

        fs::rename(&temp_write_path, &file_path).expect("Failed to rename temp file");

        tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

        let update_result = tokio::time::timeout(
            Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
            websocket.receive_json::<ServerMessage>(),
        )
        .await;

        match update_result {
            Ok(update_message) => {
                if let ServerMessage::Reload = update_message {
                    // Success
                } else {
                    panic!("Expected Reload message after temp file rename in directory mode");
                }
            }
            Err(_) => {
                panic!(
                    "Timeout waiting for WebSocket update after temp file rename in directory mode"
                );
            }
        }

        let final_response = server.get("/test1.md").await;
        assert_eq!(final_response.status_code(), 200);
        let final_body = final_response.text();
        assert!(
            final_body.contains("Updated via temp file rename"),
            "Should serve updated content after temp file rename"
        );
        assert!(
            !final_body.contains("Content of test1"),
            "Should not serve old content"
        );
    }
}
