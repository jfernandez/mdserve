# Recursive Directory Mode

## Problem

mdserve's directory mode only watches the immediate directory. Users want to
see all markdown files across an entire repo tree and navigate between them.

## Approach

**Approach A: Relative-path keys.** Change the `tracked_files` HashMap key from
bare filename to relative path (e.g. `docs/setup.md`). The URL scheme, sidebar
display, and file identity all derive from the same string.

## Data Model & Scanning

`tracked_files: HashMap<String, TrackedFile>` keys become relative paths.
Root-level files stay as `README.md` (no leading `./`).

`scan_markdown_files` becomes recursive using `std::fs` traversal. It skips:
- Directories starting with `.` (`.git`, `.github`, `.venv`, etc.)
- `node_modules`, `target`, `__pycache__`, `dist`, `build`

Key derivation everywhere changes from `path.file_name()` to
`path.strip_prefix(base_dir)`. Affected: `MarkdownState::new`,
`add_tracked_file`, `handle_markdown_file_change`.

## URL Routing & File Serving

URLs map directly to relative paths. `docs/setup.md` serves at
`/docs/setup.md`. The existing `/*filename` wildcard route captures full paths
so no router changes are needed.

Image serving resolves against `base_dir` and already handles subdirectory
paths.

## File Watcher

Watcher switches from `RecursiveMode::NonRecursive` to
`RecursiveMode::Recursive`.

`handle_file_event` derives keys via `strip_prefix(base_dir)`. Events from
excluded directories are filtered out by checking path components against the
exclusion list.

New files in subdirectories are automatically picked up and trigger a reload.

## Sidebar: Collapsible Directory Tree

Server builds a tree structure in Rust:

```
TreeEntry { name, path, is_dir, children: Vec<TreeEntry> }
```

Flat sorted paths are converted into nested `TreeEntry` values and passed to
the template. The template renders nested `<ul>` elements recursively.

- Directories: clickable header with chevron, toggles children visibility
- Files: links (same as today)
- Active file's parent directories are auto-expanded
- Expand/collapse state persisted in `localStorage`
- ~16px indent per nesting level

## Recursive Toggle

A checkbox at the top of the sidebar. The server always scans and watches
recursively. The toggle is client-side only: JS hides/shows entries at
depth > 0. Persisted in `localStorage`. Default: on (recursive).

All files remain accessible by URL regardless of toggle state.

## Exclusion List

Hardcoded skip list for directory names:

```
.git, .github, .venv, node_modules, target, __pycache__, dist, build
```

Plus any directory starting with `.`.

## Testing

- Existing tests pass (root-level keys are unchanged)
- New unit tests for recursive scanning with nested directories
- New unit tests for excluded directories
- New integration test: subdirectory files in navigation
- New unit test for tree structure builder

## Changes Summary

| Area | Change |
|-|-|
| `scan_markdown_files` | Recursive traversal with exclusion list |
| `MarkdownState` keys | Relative paths instead of bare filenames |
| `MarkdownState::new` | `strip_prefix` for key derivation |
| `add_tracked_file` | `strip_prefix` key derivation |
| `handle_markdown_file_change` | `strip_prefix` key derivation, exclusion check |
| `new_router` | `RecursiveMode::Recursive` |
| `render_markdown` | Build tree structure, pass to template |
| `templates/main.html` | Tree rendering, expand/collapse JS, recursive toggle |
| New: tree builder | Flat paths to nested `TreeEntry` |
| New: exclusion list | Hardcoded skipped directory names |

No new dependencies.
