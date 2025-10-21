use axum_test::TestServer;
use mdserve::{new_router, scan_markdown_files};
use std::fs;
use std::time::Duration;
use tempfile::{tempdir, Builder, NamedTempFile, TempDir};

const FILE_WATCH_DELAY_MS: u64 = 100;
const WEBSOCKET_TIMEOUT_SECS: u64 = 5;

const TEST_FILE_1_CONTENT: &str = "# Test 1\n\nContent of test1";
const TEST_FILE_2_CONTENT: &str = "# Test 2\n\nContent of test2";
const TEST_FILE_3_CONTENT: &str = "# Test 3\n\nContent of test3";

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

    let router =
        new_router(base_dir, tracked_files, is_directory_mode).expect("Failed to create router");

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

    let router =
        new_router(base_dir, tracked_files, is_directory_mode).expect("Failed to create router");

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
    let (server, _temp_file) = create_test_server("# Hello World\n\nThis is **bold** text.").await;

    let response = server.get("/").await;

    assert_eq!(response.status_code(), 200);
    let body = response.text();

    // Check that markdown was converted to HTML
    assert!(body.contains("<h1>Hello World</h1>"));
    assert!(body.contains("<strong>bold</strong>"));

    // Check that theme toggle is present
    assert!(body.contains("theme-toggle"));
    assert!(body.contains("openThemeModal"));

    // Check CSS variables for theming
    assert!(body.contains("--bg-color"));
    assert!(body.contains("data-theme=\"dark\""));
}

#[tokio::test]
async fn test_websocket_connection() {
    let (server, _temp_file) = create_test_server_with_http("# WebSocket Test").await;

    // Test that WebSocket endpoint exists and can be connected to
    let response = server.get_websocket("/ws").await;
    response.assert_status_switching_protocols();
}

#[tokio::test]
async fn test_file_modification_updates_via_websocket() {
    use mdserve::ServerMessage;

    let (server, temp_file) = create_test_server_with_http("# Original Content").await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Modify the file
    fs::write(&temp_file, "# Modified Content").expect("Failed to modify file");

    // Add a small delay to allow file watcher to detect change
    tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

    // Should receive reload signal via WebSocket (with timeout)
    let update_result = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    match update_result {
        Ok(update_message) => {
            if let ServerMessage::Reload = update_message {
                // Success - we received a reload signal
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

    // Check table rendering
    assert!(body.contains("<table>"));
    assert!(body.contains("<th>Name</th>"));
    assert!(body.contains("<td>John</td>"));

    // Check strikethrough
    assert!(body.contains("<del>deleted text</del>"));

    // Check code block
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
    use tempfile::tempdir;

    // Create a temporary directory
    let temp_dir = tempdir().expect("Failed to create temp dir");

    // Create a markdown file with image reference
    let md_content =
        "# Test with Image\n\n![Test Image](test.png)\n\nThis markdown references an image.";
    let md_path = temp_dir.path().join("test.md");
    fs::write(&md_path, md_content).expect("Failed to write markdown file");

    // Create a fake PNG image (1x1 pixel PNG)
    let png_data = vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8,
        0x0F, 0x00, 0x00, 0x01, 0x00, 0x01, 0x5C, 0xDD, 0x8D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let img_path = temp_dir.path().join("test.png");
    fs::write(&img_path, png_data).expect("Failed to write image file");

    // Create router with the markdown file (single-file mode)
    let base_dir = temp_dir.path().to_path_buf();
    let tracked_files = vec![md_path];
    let is_directory_mode = false;
    let router =
        new_router(base_dir, tracked_files, is_directory_mode).expect("Failed to create router");
    let server = TestServer::new(router).expect("Failed to create test server");

    // Test that markdown includes img tag
    let response = server.get("/").await;
    assert_eq!(response.status_code(), 200);
    let body = response.text();
    assert!(body.contains("<img src=\"test.png\" alt=\"Test Image\""));

    // Test that image is served correctly
    let img_response = server.get("/test.png").await;
    assert_eq!(img_response.status_code(), 200);
    assert_eq!(img_response.header("content-type"), "image/png");
    assert!(!img_response.as_bytes().is_empty());
}

#[tokio::test]
async fn test_non_image_files_not_served() {
    use tempfile::tempdir;

    // Create a temporary directory
    let temp_dir = tempdir().expect("Failed to create temp dir");

    // Create a markdown file
    let md_content = "# Test";
    let md_path = temp_dir.path().join("test.md");
    fs::write(&md_path, md_content).expect("Failed to write markdown file");

    // Create a non-image file (txt)
    let txt_path = temp_dir.path().join("secret.txt");
    fs::write(&txt_path, "secret content").expect("Failed to write txt file");

    // Create router with the markdown file (single-file mode)
    let base_dir = temp_dir.path().to_path_buf();
    let tracked_files = vec![md_path];
    let is_directory_mode = false;
    let router =
        new_router(base_dir, tracked_files, is_directory_mode).expect("Failed to create router");
    let server = TestServer::new(router).expect("Failed to create test server");

    // Test that non-image files return 404
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

    // HTML tags should be rendered, not escaped
    assert!(body.contains(r#"<div class="highlight">"#));
    assert!(body.contains(r#"<span style="color: red;">"#));
    assert!(body.contains("<p>This should be rendered as HTML, not escaped</p>"));

    // Should not contain escaped HTML
    assert!(!body.contains("&lt;div"));
    assert!(!body.contains("&gt;"));

    // Regular markdown should still work
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

    // Should contain the mermaid code block with language-mermaid class
    assert!(body.contains(r#"class="language-mermaid""#));
    assert!(body.contains("graph TD"));

    // Check for HTML-encoded or raw content (content might be HTML-encoded)
    let has_raw_content = body.contains("A[Start] --> B{Decision}");
    let has_encoded_content = body.contains("A[Start] --&gt; B{Decision}");
    assert!(
        has_raw_content || has_encoded_content,
        "Expected mermaid content not found in body"
    );

    // Should inject the local Mermaid script when mermaid blocks are detected
    assert!(body.contains(r#"<script src="/mermaid.min.js"></script>"#));

    // Should contain the Mermaid initialization functions
    assert!(body.contains("function initMermaid()"));
    assert!(body.contains("function transformMermaidCodeBlocks()"));
    assert!(body.contains("function getMermaidTheme()"));

    // Should contain regular JavaScript code block without mermaid treatment
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

    // Should NOT inject the Mermaid CDN script when no mermaid blocks are present
    assert!(!body.contains(r#"<script src="https://cdn.jsdelivr.net/npm/mermaid@11.12.0/dist/mermaid.min.js"></script>"#));

    // Should still contain the Mermaid initialization functions (they're always present)
    assert!(body.contains("function initMermaid()"));

    // Should contain regular code blocks
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

    // Should detect all three mermaid blocks
    let mermaid_occurrences = body.matches(r#"class="language-mermaid""#).count();
    assert_eq!(mermaid_occurrences, 3);

    // Should contain content from all diagrams
    assert!(body.contains("graph LR"));
    assert!(body.contains("sequenceDiagram"));
    assert!(body.contains("classDiagram"));

    // Check for HTML-encoded or raw content
    assert!(body.contains("A --&gt; B") || body.contains("A --> B"));
    assert!(body.contains("Alice-&gt;&gt;Bob") || body.contains("Alice->>Bob"));
    assert!(body.contains("Animal &lt;|-- Duck") || body.contains("Animal <|-- Duck"));

    // Should inject the Mermaid script only once
    let script_occurrences = body
        .matches(r#"<script src="/mermaid.min.js"></script>"#)
        .count();
    assert_eq!(script_occurrences, 1);
}

#[tokio::test]
async fn test_mermaid_js_etag_caching() {
    let (server, _temp_file) = create_test_server("# Test").await;

    // First request - should return 200 with ETag
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

    // Verify content is not empty
    assert!(!response.as_bytes().is_empty());

    // Second request with matching ETag - should return 304
    let response_304 = server
        .get("/mermaid.min.js")
        .add_header(
            axum::http::header::IF_NONE_MATCH,
            axum::http::HeaderValue::from_str(etag.to_str().unwrap()).unwrap(),
        )
        .await;

    assert_eq!(response_304.status_code(), 304);
    assert_eq!(response_304.header("etag"), etag);

    // Body should be empty for 304
    assert!(response_304.as_bytes().is_empty());

    // Request with non-matching ETag - should return 200
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

// Directory mode tests

#[tokio::test]
async fn test_directory_mode_serves_multiple_files() {
    let (server, _temp_dir) = create_directory_server().await;

    // Test accessing first file
    let response1 = server.get("/test1.md").await;
    assert_eq!(response1.status_code(), 200);
    let body1 = response1.text();
    assert!(body1.contains("<h1>Test 1</h1>"));
    assert!(body1.contains("Content of test1"));

    // Test accessing second file with .markdown extension
    let response2 = server.get("/test2.markdown").await;
    assert_eq!(response2.status_code(), 200);
    let body2 = response2.text();
    assert!(body2.contains("<h1>Test 2</h1>"));
    assert!(body2.contains("Content of test2"));

    // Test accessing third file
    let response3 = server.get("/test3.md").await;
    assert_eq!(response3.status_code(), 200);
    let body3 = response3.text();
    assert!(body3.contains("<h1>Test 3</h1>"));
    assert!(body3.contains("Content of test3"));
}

#[tokio::test]
async fn test_directory_mode_file_not_found() {
    let (server, _temp_dir) = create_directory_server().await;

    // Test non-existent markdown file
    let response = server.get("/nonexistent.md").await;
    assert_eq!(response.status_code(), 404);
}

#[tokio::test]
async fn test_directory_mode_has_navigation_sidebar() {
    let (server, _temp_dir) = create_directory_server().await;

    let response = server.get("/test1.md").await;
    assert_eq!(response.status_code(), 200);
    let body = response.text();

    // Check for navigation elements
    assert!(body.contains(r#"<nav class="sidebar">"#));
    assert!(body.contains(r#"<ul class="file-list">"#));

    // Check that all files appear in navigation
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

    // Verify no navigation sidebar in single-file mode
    assert!(!body.contains(r#"<nav class="sidebar">"#));
    assert!(!body.contains("<h3>Files</h3>"));
    assert!(!body.contains(r#"<ul class="file-list">"#));
}

#[tokio::test]
async fn test_directory_mode_active_file_highlighting() {
    let (server, _temp_dir) = create_directory_server().await;

    // Access test1.md and verify it's marked as active
    let response1 = server.get("/test1.md").await;
    assert_eq!(response1.status_code(), 200);
    let body1 = response1.text();

    // Verify test1.md link has active class on the same line
    assert!(
        body1.contains(r#"href="/test1.md" class="active""#),
        "test1.md link should have href and class on same line"
    );

    // Verify test1.md is the only active link
    let active_link_count = body1.matches(r#"class="active""#).count();
    assert_eq!(active_link_count, 1, "Should have exactly one active link");

    // Access test2.markdown and verify it's marked as active
    let response2 = server.get("/test2.markdown").await;
    assert_eq!(response2.status_code(), 200);
    let body2 = response2.text();

    // Verify test2.markdown link has active class on the same line
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

    // Find the positions of each file link in the HTML
    let test1_pos = body.find("test1.md").expect("test1.md not found");
    let test2_pos = body
        .find("test2.markdown")
        .expect("test2.markdown not found");
    let test3_pos = body.find("test3.md").expect("test3.md not found");

    // Verify alphabetical order
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
    use mdserve::ServerMessage;

    let (server, temp_dir) = create_directory_server_with_http().await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Modify one of the tracked files
    let test_file = temp_dir.path().join("test1.md");
    fs::write(&test_file, "# Modified Test 1\n\nContent has changed")
        .expect("Failed to modify file");

    // Add a small delay to allow file watcher to detect change
    tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

    // Should receive reload signal via WebSocket
    let update_result = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    match update_result {
        Ok(update_message) => {
            if let ServerMessage::Reload = update_message {
                // Success - we received a reload signal
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
    use mdserve::ServerMessage;

    let (server, temp_dir) = create_directory_server_with_http().await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Create a new markdown file in the directory
    let new_file = temp_dir.path().join("test4.md");
    fs::write(&new_file, "# Test 4\n\nThis is a new file").expect("Failed to create new file");

    // Add a small delay to allow file watcher to detect change
    tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

    // Should receive reload signal via WebSocket
    let update_result = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    match update_result {
        Ok(update_message) => {
            if let ServerMessage::Reload = update_message {
                // Success - we received a reload signal
            } else {
                panic!("Expected Reload message after new file creation");
            }
        }
        Err(_) => {
            panic!("Timeout waiting for WebSocket update after new file creation");
        }
    }

    // Verify the new file is accessible and appears in navigation
    let response = server.get("/test1.md").await;
    assert_eq!(response.status_code(), 200);
    let body = response.text();

    // Check that the new file appears in the navigation
    assert!(
        body.contains("test4.md"),
        "New file should appear in navigation"
    );

    // Verify the new file is accessible directly
    let new_file_response = server.get("/test4.md").await;
    assert_eq!(new_file_response.status_code(), 200);
    let new_file_body = new_file_response.text();
    assert!(new_file_body.contains("<h1>Test 4</h1>"));
    assert!(new_file_body.contains("This is a new file"));
}

#[tokio::test]
async fn test_directory_mode_file_deletion_triggers_reload() {
    use mdserve::ServerMessage;

    let (server, temp_dir) = create_directory_server_with_http().await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Delete one of the tracked files
    let file_to_delete = temp_dir.path().join("test3.md");
    fs::remove_file(&file_to_delete).expect("Failed to delete file");

    // Add a small delay to allow file watcher to detect change
    tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

    // Should receive reload signal via WebSocket
    let update_result = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    match update_result {
        Ok(update_message) => {
            if let ServerMessage::Reload = update_message {
                // Success - we received a reload signal
            } else {
                panic!("Expected Reload message after file deletion");
            }
        }
        Err(_) => {
            panic!("Timeout waiting for WebSocket update after file deletion");
        }
    }

    // Verify the deleted file no longer appears in navigation
    let response = server.get("/test1.md").await;
    assert_eq!(response.status_code(), 200);
    let body = response.text();

    // Check that the deleted file does NOT appear in navigation
    assert!(
        !body.contains("test3.md"),
        "Deleted file should not appear in navigation"
    );

    // Check that other files still appear
    assert!(body.contains("test1.md"));
    assert!(body.contains("test2.markdown"));

    // Verify the deleted file returns 404
    let deleted_file_response = server.get("/test3.md").await;
    assert_eq!(deleted_file_response.status_code(), 404);
}

#[tokio::test]
async fn test_directory_mode_file_rename_triggers_reload() {
    use mdserve::ServerMessage;

    let (server, temp_dir) = create_directory_server_with_http().await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    let old_path = temp_dir.path().join("test3.md");
    let new_path = temp_dir.path().join("test3-renamed.md");
    fs::rename(&old_path, &new_path).expect("Failed to rename file");

    tokio::time::sleep(Duration::from_millis(FILE_WATCH_DELAY_MS)).await;

    let update_result = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    match update_result {
        Ok(update_message) => {
            if let ServerMessage::Reload = update_message {
            } else {
                panic!("Expected Reload message after file rename");
            }
        }
        Err(_) => {
            panic!("Timeout waiting for WebSocket update after file rename");
        }
    }

    let response = server.get("/test1.md").await;
    assert_eq!(response.status_code(), 200);
    let body = response.text();

    assert!(
        !body.contains("test3.md"),
        "Old filename should not appear in navigation after rename"
    );

    assert!(
        body.contains("test3-renamed.md"),
        "New filename should appear in navigation after rename"
    );

    assert!(body.contains("test1.md"));
    assert!(body.contains("test2.markdown"));

    let old_file_response = server.get("/test3.md").await;
    assert_eq!(old_file_response.status_code(), 404);

    let new_file_response = server.get("/test3-renamed.md").await;
    assert_eq!(new_file_response.status_code(), 200);
    let new_file_body = new_file_response.text();
    assert!(new_file_body.contains("<h1>Test 3</h1>"));
}
