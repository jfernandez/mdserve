// Minimal lib.rs to support integration tests
pub mod app;
pub mod template;

pub use app::{new_router, scan_markdown_files, serve_markdown, ServerMessage};
pub use template::Template;
