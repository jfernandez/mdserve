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
async fn test_websocket_receives_initial_content() {
    let (server, _temp_file) = create_test_server_with_http("# Initial Content").await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Should receive initial content update
    let message: ServerMessage = websocket.receive_json().await;

    if let ServerMessage::ContentUpdate { html } = message {
        assert!(html.contains("Initial Content"));
    } else {
        panic!("Expected ContentUpdate message");
    }
}

#[tokio::test]
async fn test_file_modification_updates_via_websocket() {
    let (server, temp_file) = create_test_server_with_http("# Original Content").await;

    let mut websocket = server.get_websocket("/ws").await.into_websocket().await;

    // Receive initial content
    let initial_message: ServerMessage = websocket.receive_json().await;
    if let ServerMessage::ContentUpdate { html } = initial_message {
        assert!(html.contains("Original Content"));
    } else {
        panic!("Expected initial ContentUpdate message");
    }

    // Modify the file
    fs::write(&temp_file, "# Modified Content").expect("Failed to modify file");

    // Add a small delay to allow file watcher to detect change
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Should receive update via WebSocket (with timeout)
    let update_result = tokio::time::timeout(
        tokio::time::Duration::from_secs(5),
        websocket.receive_json::<ServerMessage>(),
    )
    .await;

    match update_result {
        Ok(update_message) => {
            if let ServerMessage::ContentUpdate { html } = update_message {
                assert!(html.contains("Modified Content"));
                assert!(!html.contains("Original Content"));
            } else {
                panic!("Expected ContentUpdate message after file modification");
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
