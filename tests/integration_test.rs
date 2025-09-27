use axum_test::TestServer;
use mdserve::{ServerMessage, new_router};
use std::fs;
use tempfile::NamedTempFile;

async fn create_test_server(content: &str) -> (TestServer, NamedTempFile) {
    let temp_file = NamedTempFile::new().expect("Failed to create temp file");
    fs::write(&temp_file, content).expect("Failed to write temp file");

    // Canonicalize the path for consistent absolute path handling
    let canonical_path = temp_file
        .path()
        .canonicalize()
        .unwrap_or_else(|_| temp_file.path().to_path_buf());
    let router = new_router(canonical_path).expect("Failed to create router");
    let server = TestServer::new(router).expect("Failed to create test server");

    (server, temp_file)
}

async fn create_test_server_with_http(content: &str) -> (TestServer, NamedTempFile) {
    let temp_file = NamedTempFile::new().expect("Failed to create temp file");
    fs::write(&temp_file, content).expect("Failed to write temp file");

    // Canonicalize the path for consistent absolute path handling
    let canonical_path = temp_file
        .path()
        .canonicalize()
        .unwrap_or_else(|_| temp_file.path().to_path_buf());
    let router = new_router(canonical_path).expect("Failed to create router");
    let server = TestServer::builder()
        .http_transport()
        .build(router)
        .expect("Failed to create test server");

    (server, temp_file)
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
async fn test_server_serves_raw_markdown() {
    let content = "# Raw Test\n\n- Item 1\n- Item 2";
    let (server, _temp_file) = create_test_server(content).await;

    let response = server.get("/raw").await;

    assert_eq!(response.status_code(), 200);
    let body = response.text();

    // Should return exact markdown content
    assert_eq!(body, content);
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

    // Add a small delay to allow file watcher to detect change
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Should receive reload signal via WebSocket (with timeout)
    let update_result = tokio::time::timeout(
        tokio::time::Duration::from_secs(5),
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

    // Create router with the markdown file
    let router = new_router(md_path).expect("Failed to create router");
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

    // Create router with the markdown file
    let router = new_router(md_path).expect("Failed to create router");
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
