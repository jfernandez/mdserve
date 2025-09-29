use anyhow::Result;
use axum::{
    Router,
    extract::{
        Path as AxumPath, State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{StatusCode, header},
    response::{Html, IntoResponse},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::SystemTime,
};
use tokio::{
    net::TcpListener,
    sync::{Mutex, broadcast, mpsc},
};
use tower_http::cors::CorsLayer;

const TEMPLATE: &str = include_str!("../template.html");
const MERMAID_JS: &str = include_str!("../mermaid.min.js");
const MATHJAX: &str = include_str!("../tex-mml-chtml.js");

type SharedMarkdownState = Arc<Mutex<MarkdownState>>;

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum ClientMessage {
    Ping,
    RequestRefresh,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum ServerMessage {
    Reload,
    Pong,
}

struct MarkdownState {
    file_path: PathBuf,
    base_dir: PathBuf,
    last_modified: SystemTime,
    cached_html: String,
    change_tx: broadcast::Sender<ServerMessage>,
}

impl MarkdownState {
    fn new(file_path: PathBuf) -> Result<Self> {
        let metadata = fs::metadata(&file_path)?;
        let last_modified = metadata.modified()?;
        let content = fs::read_to_string(&file_path)?;
        let cached_html = Self::markdown_to_html(&content);
        let (change_tx, _) = broadcast::channel::<ServerMessage>(16);

        let base_dir = file_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
            .canonicalize()
            .unwrap_or_else(|_| {
                file_path
                    .parent()
                    .unwrap_or_else(|| Path::new("."))
                    .to_path_buf()
            });

        Ok(MarkdownState {
            file_path,
            base_dir,
            last_modified,
            cached_html,
            change_tx,
        })
    }

    fn refresh_if_needed(&mut self) -> Result<bool> {
        let metadata = fs::metadata(&self.file_path)?;
        let current_modified = metadata.modified()?;

        if current_modified > self.last_modified {
            let content = fs::read_to_string(&self.file_path)?;
            self.cached_html = Self::markdown_to_html(&content);
            self.last_modified = current_modified;

            // Send reload signal to all WebSocket clients
            let _ = self.change_tx.send(ServerMessage::Reload);

            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn markdown_to_html(content: &str) -> String {
        // Create GFM options with HTML rendering enabled
        let mut options = markdown::Options::gfm();
        options.compile.allow_dangerous_html = true;

        let html_body = markdown::to_html_with_options(content, &options)
            .unwrap_or_else(|_| "Error parsing markdown".to_string());

        // Check if the HTML contains mermaid code blocks
        let has_mermaid = html_body.contains(r#"class="language-mermaid""#);

        let mermaid_assets = if has_mermaid {
            r#"<script src="/mermaid.min.js"></script>"#
        } else {
            ""
        };

        let has_mathjax = html_body.contains("$");

        let mathjax_assets = if has_mathjax {
            r#"<script src="/tex-mml-chtml.js"></script>"#
        } else {
            ""
        };

        TEMPLATE
            .replace("{CONTENT}", &html_body)
            .replace("<!-- {MERMAID_ASSETS} -->", mermaid_assets)
            .replace("<!-- {MATHJAX_ASSETS} -->", mathjax_assets)
    }
}

/// Creates a new Router for serving markdown files.
///
/// # Errors
///
/// Returns an error if:
/// - The file cannot be read or doesn't exist
/// - File metadata cannot be accessed
/// - File watcher cannot be created
/// - File watcher cannot watch the parent directory
pub fn new_router(file_path: PathBuf) -> Result<Router> {
    let state = Arc::new(Mutex::new(MarkdownState::new(file_path.clone())?));

    // Set up file watcher
    let watcher_state = state.clone();
    let watcher_file_path = file_path.clone();
    let (tx, mut rx) = mpsc::channel(100);

    let mut watcher = RecommendedWatcher::new(
        move |res: std::result::Result<Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.blocking_send(event);
            }
        },
        Config::default(),
    )?;

    // Watch the parent directory to handle atomic writes
    let watch_path = watcher_file_path.parent().unwrap_or_else(|| Path::new("."));
    watcher.watch(watch_path, RecursiveMode::NonRecursive)?;

    // Spawn task to handle events and keep watcher alive
    tokio::spawn(async move {
        let _watcher = watcher; // Move watcher into task to keep it alive
        while let Some(event) = rx.recv().await {
            // Check if any of the event paths match our file
            let file_affected = event.paths.iter().any(|path| {
                path == &watcher_file_path
                    || (path.file_name() == watcher_file_path.file_name()
                        && path.parent() == watcher_file_path.parent())
            });

            if file_affected {
                // Only process modify/create events, ignore remove events
                match event.kind {
                    notify::EventKind::Modify(_) | notify::EventKind::Create(_) => {
                        let mut state = watcher_state.lock().await;
                        let _ = state.refresh_if_needed();
                    }
                    _ => {}
                }
            }
        }
    });

    let router = Router::new()
        .route("/", get(serve_html))
        .route("/raw", get(serve_raw))
        .route("/ws", get(websocket_handler))
        .route("/mermaid.min.js", get(serve_mermaid_js))
        .route("/tex-mml-chtml.js", get(serve_mathjax))
        .route("/*path", get(serve_static_file))
        .layer(CorsLayer::permissive())
        .with_state(state);

    Ok(router)
}

/// Serves a markdown file with live reload support.
///
/// # Errors
///
/// Returns an error if:
/// - The file cannot be read or doesn't exist
/// - Cannot bind to the specified port
/// - Server fails to start
/// - Axum serve encounters an error
pub async fn serve_markdown(file_path: PathBuf, port: u16) -> Result<()> {
    let router = new_router(file_path.clone())?;

    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr).await?;

    println!("📄 Serving markdown file: {}", file_path.display());
    println!("🌐 Server running at: http://{addr}");
    println!("📝 Raw markdown at: http://{addr}/raw");
    println!("⚡ Live reload enabled - file changes will update content instantly");
    println!("\nPress Ctrl+C to stop the server");

    axum::serve(listener, router).await?;

    Ok(())
}

async fn serve_html(State(state): State<SharedMarkdownState>) -> impl IntoResponse {
    let mut state = state.lock().await;
    if let Err(e) = state.refresh_if_needed() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(format!("<h1>Error</h1><p>{e}</p>")),
        );
    }

    (StatusCode::OK, Html(state.cached_html.clone()))
}

async fn serve_raw(State(state): State<SharedMarkdownState>) -> impl IntoResponse {
    let state = state.lock().await;
    match fs::read_to_string(&state.file_path) {
        Ok(content) => (StatusCode::OK, content),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Error reading file: {e}"),
        ),
    }
}

async fn serve_mermaid_js() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/javascript")],
        MERMAID_JS,
    )
}

async fn serve_mathjax() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/javascript")],
        MATHJAX,
    )
}

async fn serve_static_file(
    AxumPath(file_path): AxumPath<String>,
    State(state): State<SharedMarkdownState>,
) -> impl IntoResponse {
    let state = state.lock().await;

    // Only serve image files
    if !is_image_file(&file_path) {
        return (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain")],
            "File not found".to_string(),
        )
            .into_response();
    }

    // Construct the full path by joining base_dir with the requested path
    let full_path = state.base_dir.join(&file_path);

    // Security check: ensure the resolved path is still within base_dir
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

            // Try to read and serve the file
            match fs::read(&canonical_path) {
                Ok(contents) => {
                    let content_type = guess_image_content_type(&file_path);
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

    // Subscribe to file change notifications
    let mut change_rx = {
        let state = state.lock().await;
        state.change_tx.subscribe()
    };

    // Spawn task to handle incoming messages from client
    let recv_task = tokio::spawn(async move {
        while let Some(msg) = receiver.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Ok(client_msg) = serde_json::from_str::<ClientMessage>(&text) {
                        match client_msg {
                            ClientMessage::Ping | ClientMessage::RequestRefresh => {
                                // Currently no special handling needed
                            }
                        }
                    }
                }
                Ok(Message::Close(_)) => break,
                _ => {}
            }
        }
    });

    // Spawn task to send messages to client
    let send_task = tokio::spawn(async move {
        // Listen for file changes
        while let Ok(reload_msg) = change_rx.recv().await {
            if let Ok(json) = serde_json::to_string(&reload_msg)
                && sender.send(Message::Text(json)).await.is_err()
            {
                break;
            }
        }
    });

    // Wait for either task to complete
    tokio::select! {
        _ = recv_task => {},
        _ = send_task => {},
    }
}
