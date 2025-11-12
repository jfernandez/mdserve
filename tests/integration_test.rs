use axum_test::TestServer;
use mdserve::{new_router, scan_markdown_files, ServerMessage};
use std::fs;
use std::time::Duration;
use tempfile::{tempdir, Builder, NamedTempFile, TempDir};

const WEBSOCKET_TIMEOUT_SECS: u64 = 5;

const TEST_FILE_1_CONTENT: &str = "# Test 1\n\nContent of test1";
const TEST_FILE_2_CONTENT: &str = "# Test 2\n\nContent of test2";
const TEST_FILE_3_CONTENT: &str = "# Test 3\n\nContent of test3";
const YAML_FRONTMATTER_CONTENT: &str = "---\ntitle: Test Post\nauthor: Name\n---\n\n# Test Post\n";
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
    let (server, temp_file) = create_test_server_with_http("# Original Content").await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Modify the file
    fs::write(&temp_file, "# Modified Content").expect("Failed to modify file");


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
    let (server, temp_dir) = create_directory_server_with_http().await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Modify one of the tracked files
    let test_file = temp_dir.path().join("test1.md");
    fs::write(&test_file, "# Modified Test 1\n\nContent has changed")
        .expect("Failed to modify file");


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
    let (server, temp_dir) = create_directory_server_with_http().await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Create a new markdown file in the directory
    let new_file = temp_dir.path().join("test4.md");
    fs::write(&new_file, "# Test 4\n\nThis is a new file").expect("Failed to create new file");


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
async fn test_editor_save_simulation_single_file_mode() {
    // Simulates neovim's save behavior: rename original to backup, create new file
    // Should NOT get 404 at any point during this sequence
    let (server, temp_file) = create_test_server_with_http("# Original\n\nOriginal content").await;

    let file_path = temp_file.path().to_path_buf();
    let backup_path = file_path.with_extension("md~");

    // Verify original content is served
    let initial_response = server.get("/").await;
    assert_eq!(initial_response.status_code(), 200);
    assert!(initial_response.text().contains("Original content"));

    // Simulate editor save: rename to backup
    fs::rename(&file_path, &backup_path).expect("Failed to rename to backup");


    // CRITICAL: File should still be accessible (not 404) even though renamed
    let during_save_response = server.get("/").await;
    assert_eq!(
        during_save_response.status_code(),
        200,
        "File should not return 404 during editor save"
    );

    // Create new file with updated content
    fs::write(&file_path, "# Updated\n\nUpdated content").expect("Failed to write new file");


    // Verify updated content is now served
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

    // Cleanup backup file
    let _ = fs::remove_file(&backup_path);
}

#[tokio::test]
async fn test_editor_save_simulation_directory_mode() {
    // Tests the same editor save behavior in directory mode
    let (server, temp_dir) = create_directory_server_with_http().await;

    let file_path = temp_dir.path().join("test1.md");
    let backup_path = temp_dir.path().join("test1.md~");

    // Verify original content
    let initial_response = server.get("/test1.md").await;
    assert_eq!(initial_response.status_code(), 200);
    assert!(initial_response.text().contains("Content of test1"));

    // Simulate editor save: rename to backup
    fs::rename(&file_path, &backup_path).expect("Failed to rename to backup");


    // CRITICAL: File should still be accessible during save
    let during_save_response = server.get("/test1.md").await;
    assert_eq!(
        during_save_response.status_code(),
        200,
        "File should not return 404 during editor save in directory mode"
    );

    // Create new file with updated content
    fs::write(&file_path, "# Test 1 Updated\n\nUpdated content").expect("Failed to write new file");


    // Verify updated content
    let final_response = server.get("/test1.md").await;
    assert_eq!(final_response.status_code(), 200);
    let final_body = final_response.text();
    assert!(
        final_body.contains("Updated content"),
        "Should serve updated content after save"
    );

    // Cleanup backup file
    let _ = fs::remove_file(&backup_path);
}

#[tokio::test]
async fn test_no_404_during_editor_save_sequence() {
    // Tests that HTTP requests during each step of the save never see 404
    let (server, temp_dir) = create_directory_server_with_http().await;
    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    let file_path = temp_dir.path().join("test1.md");
    let backup_path = temp_dir.path().join("test1.md~");

    // Step 1: Rename to backup
    fs::rename(&file_path, &backup_path).expect("Failed to rename to backup");

    // Request should still work (no 404)
    let response_after_rename = server.get("/test1.md").await;
    assert_eq!(
        response_after_rename.status_code(),
        200,
        "Should not get 404 after rename to backup"
    );

    // Step 2: Create new file
    fs::write(&file_path, "# Test 1 Updated\n\nNew content").expect("Failed to write new file");

    // Request should work with new content
    let response_after_create = server.get("/test1.md").await;
    assert_eq!(
        response_after_create.status_code(),
        200,
        "Should successfully serve after new file created"
    );
    assert!(response_after_create.text().contains("New content"));

    // Should have received reload
    let update_result = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    assert!(update_result.is_ok(), "Should receive reload after save");

    // Cleanup
    let _ = fs::remove_file(&backup_path);
}

#[tokio::test]
async fn test_yaml_frontmatter_is_stripped() {
    let (server, _temp_file) = create_test_server(YAML_FRONTMATTER_CONTENT).await;

    let response = server.get("/").await;

    assert_eq!(response.status_code(), 200);
    let body = response.text();

    // Frontmatter should be stripped
    assert!(!body.contains("title: Test Post"));
    assert!(!body.contains("author: Name"));

    // Content should still be rendered
    assert!(body.contains("<h1>Test Post</h1>"));
}

#[tokio::test]
async fn test_toml_frontmatter_is_stripped() {
    let (server, _temp_file) = create_test_server(TOML_FRONTMATTER_CONTENT).await;

    let response = server.get("/").await;

    assert_eq!(response.status_code(), 200);
    let body = response.text();

    // TOML frontmatter should be stripped
    assert!(!body.contains("title = \"Test Post\""));

    // Content should still be rendered
    assert!(body.contains("<h1>Test Post</h1>"));
}

#[tokio::test]
async fn test_temp_file_rename_triggers_reload_single_file_mode() {
    // Simulates Claude Code's save behavior: write to temp file, then rename over original
    // This pattern triggers Modify(Name(Any)) events instead of Modify(Data(Content))

    // Create server with initial file - file is now being tracked and watched
    let (server, temp_file) = create_test_server_with_http("# Original\n\nOriginal content").await;

    // Connect WebSocket to receive reload notifications
    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    let file_path = temp_file.path().to_path_buf();
    let temp_write_path = file_path.with_extension("md.tmp.12345");

    // Verify file is already tracked and serving content BEFORE the edit
    let initial_response = server.get("/").await;
    assert_eq!(initial_response.status_code(), 200);
    assert!(
        initial_response.text().contains("Original content"),
        "File should be tracked and serving content before edit"
    );

    // Simulate Claude Code's save: write to temp file
    fs::write(
        &temp_write_path,
        "# Updated\n\nUpdated content via temp file",
    )
    .expect("Failed to write temp file");


    // Rename temp file over original (atomic operation)
    fs::rename(&temp_write_path, &file_path).expect("Failed to rename temp file");


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
                panic!("Expected Reload message after temp file rename");
            }
        }
        Err(_) => {
            panic!("Timeout waiting for WebSocket update after temp file rename");
        }
    }

    // Verify updated content is now served
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
    // Tests the same temp file rename behavior in directory mode

    // Create server with directory of files - all files are now being tracked and watched
    let (server, temp_dir) = create_directory_server_with_http().await;

    // Connect WebSocket to receive reload notifications
    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    let file_path = temp_dir.path().join("test1.md");
    let temp_write_path = temp_dir.path().join("test1.md.tmp.67890");

    // Verify test1.md is already tracked and serving content BEFORE the edit
    let initial_response = server.get("/test1.md").await;
    assert_eq!(initial_response.status_code(), 200);
    assert!(
        initial_response.text().contains("Content of test1"),
        "File should be tracked and serving content before edit"
    );

    // Write to temp file
    fs::write(
        &temp_write_path,
        "# Test 1 Updated\n\nUpdated via temp file rename",
    )
    .expect("Failed to write temp file");


    // Rename temp file over original
    fs::rename(&temp_write_path, &file_path).expect("Failed to rename temp file");


    // Should receive reload signal
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
                panic!("Expected Reload message after temp file rename in directory mode");
            }
        }
        Err(_) => {
            panic!("Timeout waiting for WebSocket update after temp file rename in directory mode");
        }
    }

    // Verify updated content
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

// File rename and removal tests

#[tokio::test]
async fn test_directory_mode_file_removal_updates_sidebar() {
    // Test that when a file is removed, it disappears from the sidebar
    let (server, temp_dir) = create_directory_server_with_http().await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Verify initial state - test2.markdown exists in sidebar
    let initial_response = server.get("/test1.md").await;
    assert_eq!(initial_response.status_code(), 200);
    let initial_body = initial_response.text();
    assert!(
        initial_body.contains("test2.markdown"),
        "test2.markdown should initially be in sidebar"
    );

    // Remove test2.markdown
    let file_to_remove = temp_dir.path().join("test2.markdown");
    fs::remove_file(&file_to_remove).expect("Failed to remove file");

    // Wait for file watcher to detect removal

    // Should receive reload signal
    let update_result = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    assert!(
        update_result.is_ok(),
        "Should receive reload after file removal"
    );

    // After reload, sidebar should NOT contain the removed file
    let after_removal_response = server.get("/test1.md").await;
    assert_eq!(after_removal_response.status_code(), 200);
    let after_removal_body = after_removal_response.text();
    assert!(
        !after_removal_body.contains("test2.markdown"),
        "test2.markdown should be removed from sidebar"
    );

    // Other files should still be present
    assert!(
        after_removal_body.contains("test1.md"),
        "test1.md should still be in sidebar"
    );
    assert!(
        after_removal_body.contains("test3.md"),
        "test3.md should still be in sidebar"
    );
}

#[tokio::test]
async fn test_directory_mode_removed_file_returns_404() {
    // Test that accessing a removed file returns 404
    let (server, temp_dir) = create_directory_server_with_http().await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Verify file is accessible initially
    let initial_response = server.get("/test2.markdown").await;
    assert_eq!(initial_response.status_code(), 200);

    // Remove the file
    let file_to_remove = temp_dir.path().join("test2.markdown");
    fs::remove_file(&file_to_remove).expect("Failed to remove file");

    // Wait for file watcher

    // Wait for reload signal
    let _ = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    // Accessing the removed file should return 404
    let removed_response = server.get("/test2.markdown").await;
    assert_eq!(
        removed_response.status_code(),
        404,
        "Removed file should return 404"
    );
}

#[tokio::test]
async fn test_directory_mode_file_rename_updates_sidebar() {
    // Test that when a file is renamed, only the new name appears in sidebar
    let (server, temp_dir) = create_directory_server_with_http().await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Verify initial state - test2.markdown exists
    let initial_response = server.get("/test1.md").await;
    assert_eq!(initial_response.status_code(), 200);
    let initial_body = initial_response.text();
    assert!(
        initial_body.contains("test2.markdown"),
        "test2.markdown should initially be in sidebar"
    );
    assert!(
        !initial_body.contains("test2-renamed.md"),
        "test2-renamed.md should not exist yet"
    );

    // Rename test2.markdown to test2-renamed.md
    let old_path = temp_dir.path().join("test2.markdown");
    let new_path = temp_dir.path().join("test2-renamed.md");
    fs::rename(&old_path, &new_path).expect("Failed to rename file");

    // Wait for file watcher to detect rename

    // Should receive reload signal
    let update_result = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    assert!(
        update_result.is_ok(),
        "Should receive reload after file rename"
    );

    // After reload, sidebar should contain ONLY the new name
    let after_rename_response = server.get("/test1.md").await;
    assert_eq!(after_rename_response.status_code(), 200);
    let after_rename_body = after_rename_response.text();

    assert!(
        after_rename_body.contains("test2-renamed.md"),
        "Renamed file should appear in sidebar with new name"
    );
    assert!(
        !after_rename_body.contains("test2.markdown"),
        "Old file name should NOT appear in sidebar"
    );

    // Other files should still be present
    assert!(
        after_rename_body.contains("test1.md"),
        "test1.md should still be in sidebar"
    );
    assert!(
        after_rename_body.contains("test3.md"),
        "test3.md should still be in sidebar"
    );
}

#[tokio::test]
async fn test_directory_mode_renamed_file_accessible_with_new_name() {
    // Test that a renamed file is accessible under its new name
    let (server, temp_dir) = create_directory_server_with_http().await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Verify original file is accessible
    let original_response = server.get("/test2.markdown").await;
    assert_eq!(original_response.status_code(), 200);
    let original_body = original_response.text();
    assert!(
        original_body.contains("Content of test2"),
        "Should serve original content"
    );

    // Rename the file
    let old_path = temp_dir.path().join("test2.markdown");
    let new_path = temp_dir.path().join("test2-renamed.md");
    fs::rename(&old_path, &new_path).expect("Failed to rename file");

    // Wait for file watcher

    // Wait for reload signal
    let _ = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    // Old name should return 404
    let old_name_response = server.get("/test2.markdown").await;
    assert_eq!(
        old_name_response.status_code(),
        404,
        "Old file name should return 404"
    );

    // New name should be accessible with same content
    let new_name_response = server.get("/test2-renamed.md").await;
    assert_eq!(
        new_name_response.status_code(),
        200,
        "New file name should be accessible"
    );
    let new_name_body = new_name_response.text();
    assert!(
        new_name_body.contains("Content of test2"),
        "Should serve original content under new name"
    );
}

#[tokio::test]
async fn test_directory_mode_file_rename_preserves_content() {
    // Verify that renaming a file preserves its content
    let (server, temp_dir) = create_directory_server_with_http().await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Get content before rename
    let before_response = server.get("/test2.markdown").await;
    assert_eq!(before_response.status_code(), 200);
    let before_content = before_response.text();
    assert!(before_content.contains("Test 2"));
    assert!(before_content.contains("Content of test2"));

    // Rename the file
    let old_path = temp_dir.path().join("test2.markdown");
    let new_path = temp_dir.path().join("test2-new-name.markdown");
    fs::rename(&old_path, &new_path).expect("Failed to rename file");

    // Wait for file watcher

    // Wait for reload
    let _ = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    // Content should be preserved under new name
    let after_response = server.get("/test2-new-name.markdown").await;
    assert_eq!(after_response.status_code(), 200);
    let after_content = after_response.text();

    assert!(
        after_content.contains("Test 2"),
        "Content should be preserved after rename"
    );
    assert!(
        after_content.contains("Content of test2"),
        "Content should be preserved after rename"
    );
}

#[tokio::test]
async fn test_rename_currently_displayed_file_redirects_to_new_name() {
    // Test that renaming the currently displayed file sends a redirect message
    let (server, temp_dir) = create_directory_server_with_http().await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Load test2.markdown initially
    let initial_response = server.get("/test2.markdown").await;
    assert_eq!(initial_response.status_code(), 200);
    let initial_body = initial_response.text();
    assert!(initial_body.contains("Test 2"));

    // Rename the currently displayed file
    let old_path = temp_dir.path().join("test2.markdown");
    let new_path = temp_dir.path().join("test2-renamed.md");
    fs::rename(&old_path, &new_path).expect("Failed to rename file");

    // Wait for file watcher to detect rename

    // Should receive a FileRenamed message with old and new filenames
    let update_result = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    assert!(
        update_result.is_ok(),
        "Should receive message after file rename"
    );

    let message = update_result.unwrap();
    match message {
        ServerMessage::FileRenamed { old_name, new_name } => {
            assert_eq!(old_name, "test2.markdown");
            assert_eq!(new_name, "test2-renamed.md");
        }
        _ => panic!("Expected FileRenamed message, got {:?}", message),
    }

    // Verify new file is accessible
    let after_response = server.get("/test2-renamed.md").await;
    assert_eq!(after_response.status_code(), 200);
}

#[tokio::test]
async fn test_remove_currently_displayed_file_redirects_to_home() {
    // Test that removing the currently displayed file sends a redirect message
    let (server, temp_dir) = create_directory_server_with_http().await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Load test2.markdown initially
    let initial_response = server.get("/test2.markdown").await;
    assert_eq!(initial_response.status_code(), 200);
    let initial_body = initial_response.text();
    assert!(initial_body.contains("Test 2"));

    // Remove the currently displayed file
    let file_path = temp_dir.path().join("test2.markdown");
    fs::remove_file(&file_path).expect("Failed to remove file");

    // Wait for file watcher to detect removal

    // Should receive a FileRemoved message with the removed filename
    let update_result = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    assert!(
        update_result.is_ok(),
        "Should receive message after file removal"
    );

    let message = update_result.unwrap();
    match message {
        ServerMessage::FileRemoved { name } => {
            assert_eq!(name, "test2.markdown");
        }
        _ => panic!("Expected FileRemoved message, got {:?}", message),
    }

    // Verify removed file returns 404
    let after_response = server.get("/test2.markdown").await;
    assert_eq!(after_response.status_code(), 404);
}

#[tokio::test]
async fn test_remove_and_create_different_files_does_not_trigger_rename() {
    // Test that removing one file and creating a different file with different content
    // does NOT trigger a FileRenamed message (should be generic Reload)
    let (server, temp_dir) = create_directory_server_with_http().await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Load test2.markdown initially
    let initial_response = server.get("/test2.markdown").await;
    assert_eq!(initial_response.status_code(), 200);

    // ATOMICALLY remove test2.markdown and create a completely different file
    // We do both operations together to ensure the file watcher sees them as one change
    let removed_path = temp_dir.path().join("test2.markdown");
    let new_path = temp_dir.path().join("new-file.md");

    fs::remove_file(&removed_path).expect("Failed to remove file");
    fs::write(&new_path, "# Completely Different Content\n\nThis is a new file with different content.")
        .expect("Failed to create new file");

    // Wait for file watcher to detect changes

    // Should receive a generic Reload message (NOT FileRenamed)
    let update_result = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    assert!(
        update_result.is_ok(),
        "Should receive message after file changes"
    );

    let message = update_result.unwrap();
    // Print the message for debugging
    println!("Received message: {:?}", message);

    match message {
        ServerMessage::Reload => {
            // This is correct - different files should trigger generic reload
        }
        ServerMessage::FileRenamed { old_name, new_name } => {
            panic!(
                "BUG: Should NOT treat removal of '{}' and creation of '{}' as rename (different content)",
                old_name, new_name
            );
        }
        _ => panic!("Expected Reload message, got {:?}", message),
    }
}
#[tokio::test]
async fn test_folder_based_routing_single_level() {
    let temp_dir = tempdir().expect("Failed to create temp dir");

    // Create folder structure
    let folder1 = temp_dir.path().join("folder1");
    fs::create_dir(&folder1).expect("Failed to create folder1");
    fs::write(folder1.join("doc.md"), "# Folder1 Doc").expect("Failed to write file");
    
    fs::write(temp_dir.path().join("root.md"), "# Root Doc").expect("Failed to write root file");

    let base_dir = temp_dir.path().to_path_buf();
    let tracked_files = scan_markdown_files(&base_dir).expect("Failed to scan");
    
    let router = new_router(base_dir, tracked_files, true).expect("Failed to create router");
    let server = TestServer::new(router).expect("Failed to create test server");

    // Test accessing file in folder
    let response = server.get("/folder1/doc.md").await;
    assert_eq!(response.status_code(), 200);
    assert!(response.text().contains("Folder1 Doc"));
    
    // Test accessing root file
    let response = server.get("/root.md").await;
    assert_eq!(response.status_code(), 200);
    assert!(response.text().contains("Root Doc"));
}

#[tokio::test]
async fn test_folder_based_routing_nested_folders() {
    let temp_dir = tempdir().expect("Failed to create temp dir");

    // Create nested folder structure
    let folder1 = temp_dir.path().join("folder1");
    fs::create_dir(&folder1).expect("Failed to create folder1");
    
    let folder2 = folder1.join("folder2");
    fs::create_dir(&folder2).expect("Failed to create folder2");
    fs::write(folder2.join("nested.md"), "# Nested Doc").expect("Failed to write file");

    let base_dir = temp_dir.path().to_path_buf();
    let tracked_files = scan_markdown_files(&base_dir).expect("Failed to scan");
    
    let router = new_router(base_dir, tracked_files, true).expect("Failed to create router");
    let server = TestServer::new(router).expect("Failed to create test server");

    // Test accessing nested file
    let response = server.get("/folder1/folder2/nested.md").await;
    assert_eq!(response.status_code(), 200);
    assert!(response.text().contains("Nested Doc"));
}

#[tokio::test]
async fn test_folder_based_routing_same_filename_different_folders() {
    let temp_dir = tempdir().expect("Failed to create temp dir");

    // Create files with same name in different locations
    fs::write(temp_dir.path().join("doc.md"), "# Root Doc").expect("Failed to write root file");
    
    let folder1 = temp_dir.path().join("folder1");
    fs::create_dir(&folder1).expect("Failed to create folder1");
    fs::write(folder1.join("doc.md"), "# Folder1 Doc").expect("Failed to write folder1 file");
    
    let folder2 = temp_dir.path().join("folder2");
    fs::create_dir(&folder2).expect("Failed to create folder2");
    fs::write(folder2.join("doc.md"), "# Folder2 Doc").expect("Failed to write folder2 file");

    let base_dir = temp_dir.path().to_path_buf();
    let tracked_files = scan_markdown_files(&base_dir).expect("Failed to scan");
    
    let router = new_router(base_dir, tracked_files, true).expect("Failed to create router");
    let server = TestServer::new(router).expect("Failed to create test server");

    // Test each file can be accessed uniquely
    let response = server.get("/doc.md").await;
    assert_eq!(response.status_code(), 200);
    assert!(response.text().contains("Root Doc"));
    
    let response = server.get("/folder1/doc.md").await;
    assert_eq!(response.status_code(), 200);
    assert!(response.text().contains("Folder1 Doc"));
    
    let response = server.get("/folder2/doc.md").await;
    assert_eq!(response.status_code(), 200);
    assert!(response.text().contains("Folder2 Doc"));
}

#[tokio::test]
async fn test_folder_based_routing_404_for_nonexistent_path() {
    let temp_dir = tempdir().expect("Failed to create temp dir");

    let folder1 = temp_dir.path().join("folder1");
    fs::create_dir(&folder1).expect("Failed to create folder1");
    fs::write(folder1.join("doc.md"), "# Doc").expect("Failed to write file");

    let base_dir = temp_dir.path().to_path_buf();
    let tracked_files = scan_markdown_files(&base_dir).expect("Failed to scan");
    
    let router = new_router(base_dir, tracked_files, true).expect("Failed to create router");
    let server = TestServer::new(router).expect("Failed to create test server");

    // Test 404 for non-existent folder path
    let response = server.get("/nonexistent/doc.md").await;
    assert_eq!(response.status_code(), 404);

    // Test 404 for non-existent file in existing folder
    let response = server.get("/folder1/nonexistent.md").await;
    assert_eq!(response.status_code(), 404);
}

// ===========================
// Tree Structure Tests
// ===========================

#[tokio::test]
async fn test_tree_structure_root_files_only() {
    let temp_dir = tempdir().expect("Failed to create temp dir");

    // Create root-level files only
    fs::write(temp_dir.path().join("a.md"), "# File A").expect("Failed to write a.md");
    fs::write(temp_dir.path().join("b.md"), "# File B").expect("Failed to write b.md");
    fs::write(temp_dir.path().join("c.md"), "# File C").expect("Failed to write c.md");

    let base_dir = temp_dir.path().to_path_buf();
    let tracked_files = scan_markdown_files(&base_dir).expect("Failed to scan");

    let router = new_router(base_dir, tracked_files, true).expect("Failed to create router");
    let server = TestServer::new(router).expect("Failed to create test server");

    let response = server.get("/a.md").await;
    assert_eq!(response.status_code(), 200);
    let html = response.text();

    // Check all files are listed in sidebar
    assert!(html.contains("a.md"));
    assert!(html.contains("b.md"));
    assert!(html.contains("c.md"));

    // Should NOT have any folder elements (only file elements)
    assert!(!html.contains("class=\"folder-item\""));
    assert!(!html.contains("class=\"folder-header\""));
}

#[tokio::test]
async fn test_tree_structure_single_level_folders() {
    let temp_dir = tempdir().expect("Failed to create temp dir");

    // Create root file
    fs::write(temp_dir.path().join("root.md"), "# Root").expect("Failed to write root.md");

    // Create folder1 with files
    let folder1 = temp_dir.path().join("docs");
    fs::create_dir(&folder1).expect("Failed to create docs folder");
    fs::write(folder1.join("intro.md"), "# Intro").expect("Failed to write intro.md");
    fs::write(folder1.join("guide.md"), "# Guide").expect("Failed to write guide.md");

    // Create folder2 with files
    let folder2 = temp_dir.path().join("api");
    fs::create_dir(&folder2).expect("Failed to create api folder");
    fs::write(folder2.join("reference.md"), "# Reference").expect("Failed to write reference.md");

    let base_dir = temp_dir.path().to_path_buf();
    let tracked_files = scan_markdown_files(&base_dir).expect("Failed to scan");

    let router = new_router(base_dir, tracked_files, true).expect("Failed to create router");
    let server = TestServer::new(router).expect("Failed to create test server");

    let response = server.get("/root.md").await;
    assert_eq!(response.status_code(), 200);
    let html = response.text();

    // Check root file is listed
    assert!(html.contains("root.md"));

    // Check folder names appear (will be implemented as tree structure)
    // For now we just verify that files with folder paths appear
    assert!(html.contains("docs") || html.contains("docs/intro.md") || html.contains("docs\\intro.md"));
    assert!(html.contains("api") || html.contains("api/reference.md") || html.contains("api\\reference.md"));
}

#[tokio::test]
async fn test_tree_structure_nested_folders() {
    let temp_dir = tempdir().expect("Failed to create temp dir");

    // Create nested structure: docs/tutorials/beginner/start.md
    let tutorials = temp_dir.path().join("docs").join("tutorials");
    fs::create_dir_all(&tutorials).expect("Failed to create nested folders");

    let beginner = tutorials.join("beginner");
    fs::create_dir(&beginner).expect("Failed to create beginner folder");
    fs::write(beginner.join("start.md"), "# Start").expect("Failed to write start.md");
    fs::write(beginner.join("basics.md"), "# Basics").expect("Failed to write basics.md");

    let advanced = tutorials.join("advanced");
    fs::create_dir(&advanced).expect("Failed to create advanced folder");
    fs::write(advanced.join("expert.md"), "# Expert").expect("Failed to write expert.md");

    let base_dir = temp_dir.path().to_path_buf();
    let tracked_files = scan_markdown_files(&base_dir).expect("Failed to scan");

    let router = new_router(base_dir, tracked_files, true).expect("Failed to create router");
    let server = TestServer::new(router).expect("Failed to create test server");

    // Access a nested file
    let response = server.get("/docs/tutorials/beginner/start.md").await;
    assert_eq!(response.status_code(), 200);
    assert!(response.text().contains("Start"));

    // Check that nested paths are accessible
    let response = server.get("/docs/tutorials/advanced/expert.md").await;
    assert_eq!(response.status_code(), 200);
    assert!(response.text().contains("Expert"));
}

#[tokio::test]
async fn test_tree_structure_mixed_root_and_folders() {
    let temp_dir = tempdir().expect("Failed to create temp dir");

    // Create mix of root files and folders
    fs::write(temp_dir.path().join("readme.md"), "# README").expect("Failed to write readme.md");
    fs::write(temp_dir.path().join("changelog.md"), "# Changelog").expect("Failed to write changelog.md");

    let docs = temp_dir.path().join("docs");
    fs::create_dir(&docs).expect("Failed to create docs folder");
    fs::write(docs.join("api.md"), "# API").expect("Failed to write api.md");

    let examples = temp_dir.path().join("examples");
    fs::create_dir(&examples).expect("Failed to create examples folder");
    fs::write(examples.join("hello.md"), "# Hello").expect("Failed to write hello.md");

    let base_dir = temp_dir.path().to_path_buf();
    let tracked_files = scan_markdown_files(&base_dir).expect("Failed to scan");

    let router = new_router(base_dir, tracked_files, true).expect("Failed to create router");
    let server = TestServer::new(router).expect("Failed to create test server");

    let response = server.get("/readme.md").await;
    assert_eq!(response.status_code(), 200);
    let html = response.text();

    // Check root files are listed
    assert!(html.contains("readme.md"));
    assert!(html.contains("changelog.md"));

    // Check folder files are accessible
    let response = server.get("/docs/api.md").await;
    assert_eq!(response.status_code(), 200);

    let response = server.get("/examples/hello.md").await;
    assert_eq!(response.status_code(), 200);
}

// ===========================
// Folder Removal Tests
// ===========================

#[tokio::test]
async fn test_remove_all_files_from_folder_folder_disappears() {
    // Test that when all files are removed from a folder, the folder disappears from sidebar
    let temp_dir = tempdir().expect("Failed to create temp dir");

    // Create root file
    fs::write(temp_dir.path().join("root.md"), "# Root").expect("Failed to write root.md");

    // Create folder with files
    let docs = temp_dir.path().join("docs");
    fs::create_dir(&docs).expect("Failed to create docs folder");
    fs::write(docs.join("file1.md"), "# File 1").expect("Failed to write file1.md");
    fs::write(docs.join("file2.md"), "# File 2").expect("Failed to write file2.md");

    let base_dir = temp_dir.path().to_path_buf();
    let tracked_files = scan_markdown_files(&base_dir).expect("Failed to scan");

    let router = new_router(base_dir, tracked_files, true).expect("Failed to create router");
    let server = TestServer::builder()
        .http_transport()
        .build(router)
        .expect("Failed to create test server");

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Verify folder appears in sidebar initially
    let response = server.get("/root.md").await;
    assert_eq!(response.status_code(), 200);
    let html = response.text();
    assert!(html.contains("docs") || html.contains("data-folder-path=\"docs\""));

    // Remove all files from docs folder
    fs::remove_file(docs.join("file1.md")).expect("Failed to remove file1.md");
    fs::remove_file(docs.join("file2.md")).expect("Failed to remove file2.md");

    // Wait for reload signal
    let _ = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    // Verify folder no longer appears in sidebar
    let response = server.get("/root.md").await;
    assert_eq!(response.status_code(), 200);
    let html = response.text();
    assert!(!html.contains("data-folder-path=\"docs\""), "Empty folder should not appear in sidebar");
}

#[tokio::test]
async fn test_remove_all_files_from_nested_folder() {
    // Test that removing all files from a nested folder removes only that folder, not parent
    let temp_dir = tempdir().expect("Failed to create temp dir");

    // Create structure: root.md, docs/intro.md, docs/tutorials/tutorial1.md
    fs::write(temp_dir.path().join("root.md"), "# Root").expect("Failed to write root.md");

    let docs = temp_dir.path().join("docs");
    fs::create_dir(&docs).expect("Failed to create docs folder");
    fs::write(docs.join("intro.md"), "# Intro").expect("Failed to write intro.md");

    let tutorials = docs.join("tutorials");
    fs::create_dir(&tutorials).expect("Failed to create tutorials folder");
    fs::write(tutorials.join("tutorial1.md"), "# Tutorial 1").expect("Failed to write tutorial1.md");

    let base_dir = temp_dir.path().to_path_buf();
    let tracked_files = scan_markdown_files(&base_dir).expect("Failed to scan");

    let router = new_router(base_dir, tracked_files, true).expect("Failed to create router");
    let server = TestServer::builder()
        .http_transport()
        .build(router)
        .expect("Failed to create test server");

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Remove all files from tutorials folder
    fs::remove_file(tutorials.join("tutorial1.md")).expect("Failed to remove tutorial1.md");

    // Wait for reload signal
    let _ = tokio::time::timeout(
        Duration::from_secs(WEBSOCKET_TIMEOUT_SECS),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    // Verify tutorials folder is gone but docs folder remains
    let response = server.get("/root.md").await;
    let html = response.text();
    assert!(html.contains("data-folder-path=\"docs\""), "Parent folder should still appear");
    assert!(!html.contains("data-folder-path=\"docs/tutorials\"") && !html.contains("data-folder-path=\"docs\\tutorials\""), "Empty nested folder should not appear");
    assert!(html.contains("intro.md"), "Parent folder file should still be accessible");
}
