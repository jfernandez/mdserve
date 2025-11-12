use anyhow::Result;
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
    net::Ipv6Addr,
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
const RESCAN_DELAY_MS: u64 = 200;
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
pub enum ClientMessage {
    Ping,
    RequestRefresh,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "type")]
pub enum ServerMessage {
    Reload,
    Pong,
    FileRenamed { old_name: String, new_name: String },
    FileRemoved { name: String },
}

use std::collections::HashMap;

pub fn scan_markdown_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut md_files = Vec::new();
    scan_markdown_files_recursive(dir, &mut md_files)?;
    md_files.sort();
    Ok(md_files)
}

fn scan_markdown_files_recursive(dir: &Path, md_files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_file() && is_markdown_file(&path) {
            md_files.push(path);
        } else if path.is_dir() {
            // Recursively scan subdirectories
            scan_markdown_files_recursive(&path, md_files)?;
        }
    }

    Ok(())
}

fn is_markdown_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown"))
        .unwrap_or(false)
}

/// Calculate relative path from base_dir, canonicalizing for consistency
fn calculate_relative_path(file_path: &Path, base_dir: &Path) -> Result<String> {
    let canonical_path = file_path.canonicalize()?;
    let relative_path = canonical_path
        .strip_prefix(base_dir)
        .map_err(|_| anyhow::anyhow!("File path is not within base directory"))?
        .to_string_lossy()
        .to_string();
    Ok(relative_path)
}

/// Compare two FileTreeNode items for sorting: folders first, then files, both alphabetically
fn compare_tree_nodes(a: &FileTreeNode, b: &FileTreeNode) -> std::cmp::Ordering {
    match (a.is_folder, b.is_folder) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    }
}

struct TrackedFile {
    path: PathBuf,
    #[allow(dead_code)]  // Will be used for folder removal and root route handling
    relative_path: String,  // Path relative to base_dir (e.g., "folder/file.md")
    last_modified: SystemTime,
    html: String,
    content_hash: md5::Digest,
}

#[derive(Debug, Clone, serde::Serialize)]
struct FileTreeNode {
    name: String,           // Display name (e.g., "intro.md" or "docs")
    path: String,           // Relative path (e.g., "docs/intro.md" or "docs")
    is_folder: bool,        // true if this is a folder, false if file
    #[serde(skip_serializing_if = "Vec::is_empty")]
    children: Vec<FileTreeNode>,  // Child nodes (files and subfolders)
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
            let content_hash = md5::compute(&content);
            let relative_path = calculate_relative_path(&file_path, &base_dir)?;

            tracked_files.insert(
                relative_path.clone(),
                TrackedFile {
                    path: file_path,
                    relative_path,
                    last_modified,
                    html,
                    content_hash,
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

    fn get_file_tree(&self) -> Vec<FileTreeNode> {
        use std::collections::BTreeMap;

        // Get all file paths sorted
        let filenames = self.get_sorted_filenames();

        // Build tree structure using a nested map approach
        let mut root_files = Vec::new();
        let mut folders: BTreeMap<String, Vec<FileTreeNode>> = BTreeMap::new();

        for file_path in filenames {
            // Normalize path separators to forward slash for consistency
            let normalized_path = file_path.replace('\\', "/");
            let parts: Vec<&str> = normalized_path.split('/').collect();

            if parts.len() == 1 {
                // Root level file
                root_files.push(FileTreeNode {
                    name: parts[0].to_string(),
                    path: normalized_path.clone(),
                    is_folder: false,
                    children: Vec::new(),
                });
            } else {
                // File in a folder - build folder path incrementally
                for i in 0..parts.len() - 1 {
                    let folder_parts = &parts[0..=i];
                    let folder_path = folder_parts.join("/");

                    // Ensure this folder exists in our map
                    folders.entry(folder_path.clone()).or_default();
                }

                // Add file to its parent folder
                let parent_folder = parts[0..parts.len() - 1].join("/");
                let file_node = FileTreeNode {
                    name: parts[parts.len() - 1].to_string(),
                    path: normalized_path,
                    is_folder: false,
                    children: Vec::new(),
                };

                folders
                    .get_mut(&parent_folder)
                    .unwrap()
                    .push(file_node);
            }
        }

        // Build folder tree recursively
        fn build_folder_nodes(
            folder_path: &str,
            folders: &BTreeMap<String, Vec<FileTreeNode>>,
        ) -> FileTreeNode {
            let name = folder_path.split('/').next_back().unwrap_or(folder_path).to_string();
            let mut children = Vec::new();

            // Add files in this folder
            if let Some(files) = folders.get(folder_path) {
                children.extend(files.clone());
            }

            // Add subfolders
            let folder_prefix = format!("{}/", folder_path);
            for (sub_folder_path, _) in folders.iter() {
                if sub_folder_path.starts_with(&folder_prefix) {
                    // Check if this is a direct child (not a deeper descendant)
                    let remaining = &sub_folder_path[folder_prefix.len()..];
                    if !remaining.contains('/') {
                        let subfolder_node = build_folder_nodes(sub_folder_path, folders);
                        children.push(subfolder_node);
                    }
                }
            }

            // Sort children: folders first, then files, both alphabetically
            children.sort_by(compare_tree_nodes);

            FileTreeNode {
                name,
                path: folder_path.to_string(),
                is_folder: true,
                children,
            }
        }

        // Build root-level folder nodes
        let mut result = Vec::new();

        // Add root-level folders
        for (folder_path, _) in folders.iter() {
            // Only process top-level folders (no '/' in path)
            if !folder_path.contains('/') {
                result.push(build_folder_nodes(folder_path, &folders));
            }
        }

        // Add root-level files
        result.extend(root_files);

        // Sort result: folders first, then files, both alphabetically
        result.sort_by(compare_tree_nodes);

        result
    }

    fn refresh_file(&mut self, relative_path: &str) -> Result<()> {
        if let Some(tracked) = self.tracked_files.get_mut(relative_path) {
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
        let relative_path = calculate_relative_path(&file_path, &self.base_dir)?;

        if self.tracked_files.contains_key(&relative_path) {
            return Ok(());
        }

        let metadata = fs::metadata(&file_path)?;
        let content = fs::read_to_string(&file_path)?;
        let content_hash = md5::compute(&content);

        self.tracked_files.insert(
            relative_path.clone(),
            TrackedFile {
                path: file_path,
                relative_path,
                last_modified: metadata.modified()?,
                html: Self::markdown_to_html(&content)?,
                content_hash,
            },
        );

        Ok(())
    }

    /// Rescans the base directory and synchronizes tracked_files with the current file system state.
    /// Returns true if the file list changed (files added or removed).
    fn rescan_directory(&mut self) -> Result<bool> {
        if !self.is_directory_mode {
            return Ok(false);
        }

        // Get current files in directory
        let current_files = scan_markdown_files(&self.base_dir)?;
        let current_relative_paths: std::collections::HashSet<String> = current_files
            .iter()
            .filter_map(|p| {
                p.canonicalize().ok().and_then(|canonical| {
                    canonical
                        .strip_prefix(&self.base_dir)
                        .ok()
                        .map(|rel| rel.to_string_lossy().to_string())
                })
            })
            .collect();

        // Track relative paths that are currently tracked
        let tracked_relative_paths: std::collections::HashSet<String> =
            self.tracked_files.keys().cloned().collect();

        // Check if there are any differences
        if current_relative_paths == tracked_relative_paths {
            return Ok(false);
        }

        // Remove files that no longer exist
        self.tracked_files
            .retain(|relative_path, _| current_relative_paths.contains(relative_path));

        // Add new files
        for file_path in current_files {
            let Ok(canonical_path) = file_path.canonicalize() else {
                continue;
            };
            let Ok(rel_path) = canonical_path.strip_prefix(&self.base_dir) else {
                continue;
            };
            let relative_path = rel_path.to_string_lossy().to_string();

            if self.tracked_files.contains_key(&relative_path) {
                continue;
            }

            // Try to add new file, ignore errors for individual files
            let Ok(metadata) = fs::metadata(&file_path) else {
                continue;
            };
            let Ok(content) = fs::read_to_string(&file_path) else {
                continue;
            };
            let Ok(html) = Self::markdown_to_html(&content) else {
                continue;
            };
            let Ok(last_modified) = metadata.modified() else {
                continue;
            };
            let content_hash = md5::compute(&content);

            self.tracked_files.insert(
                relative_path.clone(),
                TrackedFile {
                    path: file_path,
                    relative_path,
                    last_modified,
                    html,
                    content_hash,
                },
            );
        }

        Ok(true)
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

    let mut state_guard = state.lock().await;

    let Ok(relative_path) = calculate_relative_path(path, &state_guard.base_dir) else {
        return;
    };

    // If file is already tracked, refresh its content
    if state_guard.tracked_files.contains_key(&relative_path) {
        if state_guard.refresh_file(&relative_path).is_ok() {
            let _ = state_guard.change_tx.send(ServerMessage::Reload);
        }
    } else if state_guard.is_directory_mode {
        // New file in directory mode - add and reload
        if state_guard.add_tracked_file(path.to_path_buf()).is_ok() {
            let _ = state_guard.change_tx.send(ServerMessage::Reload);
        }
    }
}

enum FileChangeType {
    Renamed { old_name: String, new_name: String },
    Removed { name: String },
    Other,
}

fn detect_file_change(
    old_files: &std::collections::HashSet<String>,
    new_files: &std::collections::HashSet<String>,
    old_tracked_files: &HashMap<String, md5::Digest>,
    new_tracked_files: &HashMap<String, TrackedFile>,
) -> FileChangeType {
    let added: Vec<_> = new_files.difference(old_files).collect();
    let removed: Vec<_> = old_files.difference(new_files).collect();

    // Detect rename: exactly one file added and one removed with matching content hashes
    if let ([new_name], [old_name]) = (added.as_slice(), removed.as_slice()) {
        // Verify content is the same by comparing hashes
        if let (Some(old_hash), Some(new_file)) = (
            old_tracked_files.get(*old_name),
            new_tracked_files.get(*new_name),
        ) {
            if *old_hash == new_file.content_hash {
                return FileChangeType::Renamed {
                    old_name: (*old_name).clone(),
                    new_name: (*new_name).clone(),
                };
            }
        }
    }

    // Detect removal: at least one file removed
    if let Some(&first_removed) = removed.first() {
        return FileChangeType::Removed {
            name: first_removed.clone(),
        };
    }

    FileChangeType::Other
}

fn send_change_message(
    change_type: FileChangeType,
    tx: &broadcast::Sender<ServerMessage>,
) {
    let message = match change_type {
        FileChangeType::Renamed { old_name, new_name } => {
            ServerMessage::FileRenamed { old_name, new_name }
        }
        FileChangeType::Removed { name } => ServerMessage::FileRemoved { name },
        FileChangeType::Other => ServerMessage::Reload,
    };

    let _ = tx.send(message);
}

async fn rescan_and_detect_changes(state: &SharedMarkdownState) {
    let (old_files, old_hashes) = {
        let guard = state.lock().await;
        let files = guard.tracked_files.keys().cloned().collect();
        let hashes: HashMap<String, md5::Digest> = guard
            .tracked_files
            .iter()
            .map(|(k, v)| (k.clone(), v.content_hash))
            .collect();
        (files, hashes)
    };

    let mut guard = state.lock().await;

    let Ok(changed) = guard.rescan_directory() else {
        return;
    };

    if !changed {
        return;
    }

    let new_files: std::collections::HashSet<String> =
        guard.tracked_files.keys().cloned().collect();

    let change_type = detect_file_change(&old_files, &new_files, &old_hashes, &guard.tracked_files);
    send_change_message(change_type, &guard.change_tx);
}

/// Schedules a delayed rescan for directory mode to handle editor save sequences.
/// Editors often rename files to backups, then create new files - we want both operations to complete.
fn schedule_delayed_rescan(state: &SharedMarkdownState) {
    let state_clone = state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(RESCAN_DELAY_MS)).await;
        rescan_and_detect_changes(&state_clone).await;
    });
}

async fn handle_rename_event(
    mode: notify::event::RenameMode,
    paths: &[PathBuf],
    state: &SharedMarkdownState,
) {
    use notify::event::RenameMode;

    let is_dir_mode = state.lock().await.is_directory_mode;
    if is_dir_mode {
        schedule_delayed_rescan(state);
        return;
    }

    match mode {
        RenameMode::Both => {
            let Some(new_path) = paths.get(1) else { return };
            handle_markdown_file_change(new_path, state).await;
        }
        RenameMode::To => {
            let Some(path) = paths.first() else { return };
            handle_markdown_file_change(path, state).await;
        }
        RenameMode::Any => {
            let Some(path) = paths.first() else { return };
            if !path.exists() {
                return;
            }
            handle_markdown_file_change(path, state).await;
        }
        RenameMode::From | RenameMode::Other => {}
    }
}

async fn handle_md_create_or_modify(path: &Path, state: &SharedMarkdownState) {
    handle_markdown_file_change(path, state).await;
}

async fn handle_md_remove(_path: &Path, state: &SharedMarkdownState) {
    let is_dir_mode = state.lock().await.is_directory_mode;
    if !is_dir_mode {
        return;
    }

    schedule_delayed_rescan(state);
}

async fn handle_image_change(state: &SharedMarkdownState) {
    let guard = state.lock().await;
    let _ = guard.change_tx.send(ServerMessage::Reload);
}

async fn handle_file_event(event: Event, state: &SharedMarkdownState) {
    use notify::EventKind::{Create, Modify, Remove};
    use notify::event::ModifyKind;

    match event.kind {
        Modify(ModifyKind::Name(rename_mode)) => {
            handle_rename_event(rename_mode, &event.paths, state).await;
        }
        _ => {
            for path in &event.paths {
                if is_markdown_file(path) {
                    match event.kind {
                        Create(_) | Modify(ModifyKind::Data(_)) => {
                            handle_md_create_or_modify(path, state).await;
                        }
                        Remove(_) => {
                            handle_md_remove(path, state).await;
                        }
                        _ => {}
                    }
                } else if path.is_file() && is_image_file(path.to_str().unwrap_or("")) {
                    match event.kind {
                        Modify(_) | Create(_) | Remove(_) => {
                            handle_image_change(state).await;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

/// Creates a new Router for serving markdown files.
///
/// # Errors
///
/// Returns an error if:
/// - Files cannot be read or don't exist
/// - File metadata cannot be accessed
/// - File watcher cannot be created
/// - File watcher cannot watch the base directory
pub fn new_router(
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

    // Watch recursively to detect file changes in subdirectories
    watcher.watch(&base_dir, RecursiveMode::Recursive)?;

    tokio::spawn(async move {
        let _watcher = watcher;
        while let Some(event) = rx.recv().await {
            handle_file_event(event, &watcher_state).await;
        }
    });

    let router = Router::new()
        .route("/", get(serve_html_root))
        .route("/ws", get(websocket_handler))
        .route("/__health", get(server_health))
        .route("/mermaid.min.js", get(serve_mermaid_js))
        .route("/*path", get(serve_file))
        .layer(CorsLayer::permissive())
        .with_state(state);

    Ok(router)
}

/// Serves markdown files with live reload support.
///
/// # Errors
///
/// Returns an error if:
/// - Files cannot be read or don't exist
/// - Cannot bind to the specified host address
/// - Server fails to start
/// - Axum serve encounters an error
pub async fn serve_markdown(
    base_dir: PathBuf,
    tracked_files: Vec<PathBuf>,
    is_directory_mode: bool,
    hostname: impl AsRef<str>,
    port: u16,
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

async fn serve_html_root(State(state): State<SharedMarkdownState>) -> impl IntoResponse {
    let mut state = state.lock().await;

    let relative_path = match state.get_sorted_filenames().into_iter().next() {
        Some(name) => name,
        None => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html("No files available to serve".to_string()),
            );
        }
    };

    let _ = state.refresh_file(&relative_path);

    render_markdown(&state, &relative_path).await
}

async fn serve_file(
    AxumPath(path): AxumPath<String>,
    State(state): State<SharedMarkdownState>,
) -> axum::response::Response {
    // Strip leading slash from path (/*path includes it)
    let relative_path = path.strip_prefix('/').unwrap_or(&path);

    if relative_path.ends_with(".md") || relative_path.ends_with(".markdown") {
        let mut state = state.lock().await;

        if !state.tracked_files.contains_key(relative_path) {
            return (StatusCode::NOT_FOUND, Html("File not found".to_string())).into_response();
        }

        let _ = state.refresh_file(relative_path);

        let (status, html) = render_markdown(&state, relative_path).await;
        (status, html).into_response()
    } else if is_image_file(relative_path) {
        serve_static_file_inner(relative_path.to_string(), state).await
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
        let file_tree = state.get_file_tree();
        let files = Value::from_serialize(&file_tree);

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

async fn server_health() -> impl IntoResponse {
    (StatusCode::OK, "ready")
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
    fn test_scan_markdown_files_scans_subdirectories_recursively() {
        let temp_dir = tempdir().expect("Failed to create temp dir");

        fs::write(temp_dir.path().join("root.md"), "# Root").expect("Failed to write");

        let sub_dir = temp_dir.path().join("subdir");
        fs::create_dir(&sub_dir).expect("Failed to create subdir");
        fs::write(sub_dir.join("nested.md"), "# Nested").expect("Failed to write");

        let result = scan_markdown_files(temp_dir.path()).expect("Failed to scan");

        assert_eq!(result.len(), 2);
        let filenames: Vec<_> = result
            .iter()
            .map(|p| p.strip_prefix(temp_dir.path()).unwrap().to_str().unwrap())
            .collect();

        assert!(filenames.contains(&"root.md"));
        assert!(filenames.contains(&"subdir/nested.md") || filenames.contains(&"subdir\\nested.md"));
    }

    #[test]
    fn test_scan_markdown_files_with_nested_folders() {
        let temp_dir = tempdir().expect("Failed to create temp dir");

        // Create: root.md, folder1/file1.md, folder1/folder2/file2.md, folder3/file3.md
        fs::write(temp_dir.path().join("root.md"), "# Root").expect("Failed to write");

        let folder1 = temp_dir.path().join("folder1");
        fs::create_dir(&folder1).expect("Failed to create folder1");
        fs::write(folder1.join("file1.md"), "# File 1").expect("Failed to write");

        let folder2 = folder1.join("folder2");
        fs::create_dir(&folder2).expect("Failed to create folder2");
        fs::write(folder2.join("file2.md"), "# File 2").expect("Failed to write");

        let folder3 = temp_dir.path().join("folder3");
        fs::create_dir(&folder3).expect("Failed to create folder3");
        fs::write(folder3.join("file3.md"), "# File 3").expect("Failed to write");

        let result = scan_markdown_files(temp_dir.path()).expect("Failed to scan");

        assert_eq!(result.len(), 4);
    }

    #[test]
    fn test_scan_markdown_files_ignores_empty_folders() {
        let temp_dir = tempdir().expect("Failed to create temp dir");

        fs::write(temp_dir.path().join("root.md"), "# Root").expect("Failed to write");

        // Create empty folder
        let empty_folder = temp_dir.path().join("empty");
        fs::create_dir(&empty_folder).expect("Failed to create empty folder");

        // Create folder with non-markdown files only
        let non_md_folder = temp_dir.path().join("non_md");
        fs::create_dir(&non_md_folder).expect("Failed to create non_md folder");
        fs::write(non_md_folder.join("file.txt"), "Text file").expect("Failed to write");

        let result = scan_markdown_files(temp_dir.path()).expect("Failed to scan");

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].file_name().unwrap().to_str().unwrap(), "root.md");
    }

    #[test]
    fn test_scan_markdown_files_handles_filename_conflicts() {
        // Test that files with the same name in different folders are both tracked
        let temp_dir = tempdir().expect("Failed to create temp dir");

        // Create file.md in root
        fs::write(temp_dir.path().join("file.md"), "# Root File").expect("Failed to write");

        // Create file.md in folder1
        let folder1 = temp_dir.path().join("folder1");
        fs::create_dir(&folder1).expect("Failed to create folder1");
        fs::write(folder1.join("file.md"), "# Folder1 File").expect("Failed to write");

        // Create file.md in folder2
        let folder2 = temp_dir.path().join("folder2");
        fs::create_dir(&folder2).expect("Failed to create folder2");
        fs::write(folder2.join("file.md"), "# Folder2 File").expect("Failed to write");

        let result = scan_markdown_files(temp_dir.path()).expect("Failed to scan");

        // Should find all 3 files with the same name
        assert_eq!(result.len(), 3);

        // Verify we can distinguish between them by their paths
        let paths: Vec<_> = result
            .iter()
            .map(|p| p.strip_prefix(temp_dir.path()).unwrap().to_str().unwrap())
            .collect();

        assert!(paths.contains(&"file.md"));
        assert!(paths.contains(&"folder1/file.md") || paths.contains(&"folder1\\file.md"));
        assert!(paths.contains(&"folder2/file.md") || paths.contains(&"folder2\\file.md"));
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

    #[tokio::test]
    async fn test_file_watcher_detects_new_files_in_subdirectories() {
        use axum_test::TestServer;
        use std::time::Duration;
        use tokio::time::{sleep, timeout};

        let temp_dir = tempdir().expect("Failed to create temp dir");

        // Create initial file that will be tracked
        fs::write(temp_dir.path().join("initial.md"), "# Initial").expect("Failed to write initial.md");

        // Start the server with directory mode
        let router = new_router(
            temp_dir.path().to_path_buf(),
            vec![],
            true,
        ).expect("Failed to create router");

        let server = TestServer::new(router).expect("Failed to create test server");

        // Poll until server is ready using the lightweight health endpoint.
        let poll_result = timeout(Duration::from_secs(2), async {
            loop {
                let response = server.get("/__health").await;
                if response.status_code() == 200 {
                    break;
                }
                sleep(Duration::from_millis(10)).await;
            }
        }).await;
        assert!(poll_result.is_ok(), "Server should be ready within 2 seconds");

        // Create a new subdirectory after the server has started
        let subfolder = temp_dir.path().join("subfolder");
        fs::create_dir(&subfolder).expect("Failed to create subfolder");

        // Create a new file in the subdirectory
        fs::write(subfolder.join("new.md"), "# New File").expect("Failed to write new.md");

        // Poll until the new file is detected and accessible by the file watcher
        let poll_result = timeout(Duration::from_secs(3), async {
            loop {
                let response = server.get("/subfolder/new.md").await;
                if response.status_code() == 200 {
                    break;
                }
                sleep(Duration::from_millis(50)).await;
            }
        }).await;
        assert!(poll_result.is_ok(), "File watcher should detect new files in subdirectories within 3 seconds");

        // Verify the file list includes the new file
        let response = server.get("/").await;
        let body = response.text();
        assert!(body.contains("new.md"), "Navigation should include the new file from subdirectory");
    }
}
