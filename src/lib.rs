// Minimal lib.rs to support integration tests
pub mod app;
pub use app::{new_router, serve_markdown, ServerMessage};
