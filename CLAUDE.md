# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

mdserve is a markdown preview server built as a companion for AI coding agents.
It renders markdown to HTML with live reload via WebSocket, supporting both
single-file and directory modes through a unified architecture.

## Build and test

```bash
cargo build --release
cargo test                            # all tests (unit + integration, all in src/app.rs)
cargo test test_server_starts         # run a single test by name substring
```

Rust 1.82+, 2021 edition. Templates are embedded at compile time via
`minijinja-embed` -- changes to `templates/` require a rebuild. The Mermaid JS
bundle in `static/js/` is also embedded via `include_str!` in `app.rs`.

## Architecture

Three source files, one template, one build script:

- `src/main.rs` -- CLI parsing (clap derive), determines single-file vs
  directory mode, calls `serve_markdown`
- `src/app.rs` -- everything else: Axum router, handlers, state management,
  file watcher, WebSocket, static file serving, and all tests
- `src/lib.rs` -- currently empty (markdown rendering moved into `MarkdownState`)
- `templates/main.html` -- single MiniJinja template handling both modes via
  conditional blocks (`{% if show_navigation %}`)
- `build.rs` -- `minijinja_embed::embed_templates!` for compile-time template
  embedding

Key types in `app.rs`:
- `MarkdownState` -- central state: base_dir, tracked files HashMap, directory
  mode flag, broadcast channel for reload signals
- `TrackedFile` -- per-file: path, last_modified timestamp, pre-rendered HTML
- `SharedMarkdownState` = `Arc<Mutex<MarkdownState>>`

Data flow: file system events (notify crate, non-recursive) -> mpsc channel ->
`handle_file_event` -> state update + broadcast `ServerMessage::Reload` ->
WebSocket clients -> `window.location.reload()`

The `/:filename` route serves both markdown files and images (checked by
extension). Directory traversal is blocked by rejecting paths containing `/`
and by canonicalize + starts_with check for static files.

## Design constraints

- **Agent-companion scope.** Not a documentation platform or configurable server.
- **Zero config.** `mdserve file.md` must work with no flags or config files.
- **Non-recursive.** Directory mode watches only the immediate directory.
- **Pre-rendered in memory.** All tracked files rendered to HTML on startup and
  on change. Serving is always from memory, never from disk.
- **Minimal client-side JS.** Client JS handles theme selection, sidebar toggle,
  Mermaid rendering, and WebSocket reload only.
- **No file removal on delete events.** Editors like neovim save via
  rename-to-backup then create-new. Removing on delete would cause transient
  404s. Files stay tracked even after `Remove` events.

## Changelog

Generated with [git-cliff](https://git-cliff.org/) using `cliff.toml`:

```bash
git cliff -o CHANGELOG.md
```

## Commits

Use conventional commits: `type: lowercase description` (e.g. `feat:`, `fix:`,
`chore:`, `docs:`, `refactor:`, `test:`). No scopes, no emojis. Subject line
max 72 chars, imperative mood. Body optional, wrap at 72 chars, explain why not
what.
