// Minimal lib.rs to support integration tests
pub mod app;
pub use app::{ServerMessage, new_router, serve_markdown};
