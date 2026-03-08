# Recursive Directory Mode Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make mdserve's directory mode recursively scan subdirectories for markdown files, display them in a collapsible tree sidebar, and allow toggling between flat/recursive views.

**Architecture:** Change `tracked_files` HashMap keys from bare filenames to relative paths (e.g. `docs/setup.md`). Recursive `std::fs` traversal with hardcoded exclusion list. Server always scans recursively; a client-side toggle hides/shows nested entries. Collapsible directory tree built in Rust as `TreeEntry` structs, rendered via MiniJinja recursive loop.

**Tech Stack:** Rust, Axum, MiniJinja (recursive `{% for %}` loops), vanilla JS for expand/collapse + toggle persistence in localStorage.

---

### Task 1: Add exclusion list and `is_excluded_dir` helper

**Files:**
- Modify: `src/app.rs` (add constant + function after line 35, before the `SharedMarkdownState` type alias)

**Step 1: Write the failing tests**

Add these tests inside `mod tests` in `src/app.rs`:

```rust
#[test]
fn test_is_excluded_dir() {
    assert!(is_excluded_dir("node_modules"));
    assert!(is_excluded_dir("target"));
    assert!(is_excluded_dir(".git"));
    assert!(is_excluded_dir(".venv"));
    assert!(is_excluded_dir("__pycache__"));
    assert!(is_excluded_dir("dist"));
    assert!(is_excluded_dir("build"));
    assert!(is_excluded_dir(".github"));
    assert!(is_excluded_dir(".hidden_anything"));

    assert!(!is_excluded_dir("docs"));
    assert!(!is_excluded_dir("src"));
    assert!(!is_excluded_dir("my_module"));
    assert!(!is_excluded_dir("README.md"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_is_excluded_dir`
Expected: FAIL — `is_excluded_dir` not found

**Step 3: Write minimal implementation**

Add after the `MERMAID_ETAG` constant (around line 34) in `src/app.rs`:

```rust
const EXCLUDED_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "__pycache__",
    "dist",
    "build",
];

fn is_excluded_dir(name: &str) -> bool {
    name.starts_with('.') || EXCLUDED_DIRS.contains(&name)
}
```

**Step 4: Run test to verify it passes**

Run: `cargo test test_is_excluded_dir`
Expected: PASS

**Step 5: Commit**

```bash
git add src/app.rs
git commit -m "feat: add directory exclusion list for recursive scanning"
```

---

### Task 2: Make `scan_markdown_files` recursive

**Files:**
- Modify: `src/app.rs` — rewrite `scan_markdown_files` (lines 59-74)

**Step 1: Update existing test to expect recursive results**

Replace `test_scan_markdown_files_ignores_subdirectories` with a test that expects subdirectory files to be found:

```rust
#[test]
fn test_scan_markdown_files_finds_subdirectory_files() {
    let temp_dir = tempdir().expect("Failed to create temp dir");

    fs::write(temp_dir.path().join("root.md"), "# Root").expect("Failed to write");

    let sub_dir = temp_dir.path().join("docs");
    fs::create_dir(&sub_dir).expect("Failed to create subdir");
    fs::write(sub_dir.join("nested.md"), "# Nested").expect("Failed to write");

    let deep_dir = sub_dir.join("api");
    fs::create_dir(&deep_dir).expect("Failed to create deep dir");
    fs::write(deep_dir.join("reference.md"), "# Reference").expect("Failed to write");

    let result = scan_markdown_files(temp_dir.path()).expect("Failed to scan");

    assert_eq!(result.len(), 3);

    let rel_paths: Vec<_> = result
        .iter()
        .map(|p| {
            p.strip_prefix(temp_dir.path())
                .unwrap()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    assert!(rel_paths.contains(&"root.md".to_string()));
    assert!(rel_paths.contains(&"docs/nested.md".to_string()));
    assert!(rel_paths.contains(&"docs/api/reference.md".to_string()));
}
```

**Step 2: Write test for exclusion during scanning**

```rust
#[test]
fn test_scan_markdown_files_excludes_hidden_and_build_dirs() {
    let temp_dir = tempdir().expect("Failed to create temp dir");

    fs::write(temp_dir.path().join("root.md"), "# Root").expect("Failed to write");

    // These directories should be excluded
    for dir_name in &[".git", "node_modules", "target", ".venv", "__pycache__"] {
        let excluded = temp_dir.path().join(dir_name);
        fs::create_dir(&excluded).expect("Failed to create dir");
        fs::write(excluded.join("hidden.md"), "# Hidden").expect("Failed to write");
    }

    // This directory should be included
    let docs = temp_dir.path().join("docs");
    fs::create_dir(&docs).expect("Failed to create docs dir");
    fs::write(docs.join("visible.md"), "# Visible").expect("Failed to write");

    let result = scan_markdown_files(temp_dir.path()).expect("Failed to scan");

    assert_eq!(result.len(), 2);
    let rel_paths: Vec<_> = result
        .iter()
        .map(|p| {
            p.strip_prefix(temp_dir.path())
                .unwrap()
                .to_string_lossy()
                .to_string()
        })
        .collect();
    assert!(rel_paths.contains(&"root.md".to_string()));
    assert!(rel_paths.contains(&"docs/visible.md".to_string()));
}
```

**Step 3: Run tests to verify they fail**

Run: `cargo test test_scan_markdown_files_finds_subdirectory_files test_scan_markdown_files_excludes_hidden`
Expected: FAIL — old implementation doesn't recurse

**Step 4: Rewrite `scan_markdown_files`**

Replace the function body:

```rust
pub(crate) fn scan_markdown_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut md_files = Vec::new();
    scan_markdown_files_recursive(dir, &mut md_files)?;
    md_files.sort();
    Ok(md_files)
}

fn scan_markdown_files_recursive(dir: &Path, md_files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            let dir_name = entry.file_name();
            let dir_name = dir_name.to_string_lossy();
            if !is_excluded_dir(&dir_name) {
                scan_markdown_files_recursive(&path, md_files)?;
            }
        } else if path.is_file() && is_markdown_file(&path) {
            md_files.push(path);
        }
    }
    Ok(())
}
```

**Step 5: Run all scan tests to verify they pass**

Run: `cargo test test_scan_markdown`
Expected: ALL PASS

**Step 6: Delete the old `test_scan_markdown_files_ignores_subdirectories` test**

It tested the old non-recursive behavior. Remove it.

**Step 7: Run full test suite**

Run: `cargo test`
Expected: PASS (existing tests use root-level files only, so keys are unchanged)

**Step 8: Commit**

```bash
git add src/app.rs
git commit -m "feat: make scan_markdown_files recursive with exclusion list"
```

---

### Task 3: Change key derivation to relative paths

**Files:**
- Modify: `src/app.rs` — `MarkdownState::new`, `add_tracked_file`, `handle_markdown_file_change`

**Step 1: Write a test for relative-path keys**

```rust
#[tokio::test]
async fn test_directory_mode_serves_subdirectory_files() {
    let temp_dir = tempdir().expect("Failed to create temp dir");

    fs::write(temp_dir.path().join("root.md"), "# Root\n\nRoot content")
        .expect("Failed to write");

    let sub_dir = temp_dir.path().join("docs");
    fs::create_dir(&sub_dir).expect("Failed to create subdir");
    fs::write(sub_dir.join("guide.md"), "# Guide\n\nGuide content")
        .expect("Failed to write");

    let base_dir = temp_dir.path().to_path_buf();
    let tracked_files = scan_markdown_files(&base_dir).expect("Failed to scan");
    let router = new_router(base_dir, tracked_files, true).expect("Failed to create router");
    let server = TestServer::new(router).expect("Failed to create test server");

    // Root file served at /root.md
    let response = server.get("/root.md").await;
    assert_eq!(response.status_code(), 200);
    assert!(response.text().contains("Root content"));

    // Subdirectory file served at /docs/guide.md
    let response = server.get("/docs/guide.md").await;
    assert_eq!(response.status_code(), 200);
    assert!(response.text().contains("Guide content"));
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test test_directory_mode_serves_subdirectory_files`
Expected: FAIL — `docs/guide.md` returns 404 because key is `guide.md` not `docs/guide.md`

**Step 3: Update `MarkdownState::new` key derivation**

Change the key derivation inside `MarkdownState::new` from:

```rust
let filename = file_path.file_name().unwrap().to_string_lossy().to_string();
```

to:

```rust
let filename = file_path
    .strip_prefix(&base_dir)
    .unwrap_or(&file_path)
    .to_string_lossy()
    .to_string();
```

**Step 4: Update `add_tracked_file` key derivation**

Change from:

```rust
let filename = file_path.file_name().unwrap().to_string_lossy().to_string();
```

to:

```rust
let filename = file_path
    .strip_prefix(&self.base_dir)
    .unwrap_or(&file_path)
    .to_string_lossy()
    .to_string();
```

**Step 5: Update `handle_markdown_file_change` key derivation**

Replace the current key derivation block:

```rust
let filename = path.file_name().and_then(|n| n.to_str()).map(String::from);
let Some(filename) = filename else {
    return;
};

let mut state_guard = state.lock().await;
```

with:

```rust
let mut state_guard = state.lock().await;

let filename = path
    .strip_prefix(&state_guard.base_dir)
    .ok()
    .map(|p| p.to_string_lossy().to_string());
let Some(filename) = filename else {
    return;
};
```

**Step 6: Run tests**

Run: `cargo test`
Expected: ALL PASS including the new subdirectory test

**Step 7: Commit**

```bash
git add src/app.rs
git commit -m "feat: use relative paths as tracked file keys"
```

---

### Task 4: Switch watcher to recursive mode with event filtering

**Files:**
- Modify: `src/app.rs` — `new_router` (line ~303), `handle_file_event`

**Step 1: Change watcher to recursive**

In `new_router`, change:

```rust
watcher.watch(&base_dir, RecursiveMode::NonRecursive)?;
```

to:

```rust
watcher.watch(&base_dir, RecursiveMode::Recursive)?;
```

**Step 2: Add exclusion filtering to `handle_file_event`**

Add a helper function:

```rust
fn is_path_excluded(path: &Path, base_dir: &Path) -> bool {
    if let Ok(rel) = path.strip_prefix(base_dir) {
        rel.components().any(|c| {
            if let std::path::Component::Normal(name) = c {
                is_excluded_dir(&name.to_string_lossy())
            } else {
                false
            }
        })
    } else {
        false
    }
}
```

At the top of `handle_file_event`, before the `match event.kind`, add an early return. We need access to `base_dir` from state for this check. Modify the function to check each path:

In the `_ =>` branch of `handle_file_event` where it iterates `for path in &event.paths`, add at the start of the loop body:

```rust
// Skip events from excluded directories
{
    let state_guard = state.lock().await;
    if is_path_excluded(path, &state_guard.base_dir) {
        continue;
    }
}
```

Note: This acquires and releases the lock just for the check. The later code also acquires the lock. This is fine since the check is fast.

Actually, a simpler approach — check path components directly without needing base_dir. Any path containing an excluded directory name as a component should be skipped:

```rust
fn has_excluded_component(path: &Path) -> bool {
    path.components().any(|c| {
        if let std::path::Component::Normal(name) = c {
            is_excluded_dir(&name.to_string_lossy())
        } else {
            false
        }
    })
}
```

Add this check at the start of `handle_markdown_file_change`:

```rust
async fn handle_markdown_file_change(path: &Path, state: &SharedMarkdownState) {
    if !is_markdown_file(path) || has_excluded_component(path) {
        return;
    }
    // ... rest unchanged
}
```

And in `handle_file_event`'s image handling branch, add the same check before processing.

**Step 3: Write a test for `has_excluded_component`**

```rust
#[test]
fn test_has_excluded_component() {
    assert!(has_excluded_component(Path::new("/tmp/project/node_modules/README.md")));
    assert!(has_excluded_component(Path::new("/tmp/project/.git/config")));
    assert!(has_excluded_component(Path::new("/tmp/project/target/debug/build.md")));
    assert!(has_excluded_component(Path::new("/tmp/project/.hidden/notes.md")));

    assert!(!has_excluded_component(Path::new("/tmp/project/docs/guide.md")));
    assert!(!has_excluded_component(Path::new("/tmp/project/src/main.rs")));
    assert!(!has_excluded_component(Path::new("/tmp/project/README.md")));
}
```

**Step 4: Run tests**

Run: `cargo test`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add src/app.rs
git commit -m "feat: switch to recursive file watcher with exclusion filtering"
```

---

### Task 5: Build tree structure for template

**Files:**
- Modify: `src/app.rs` — add `TreeEntry` struct and `build_file_tree` function

**Step 1: Write tests for tree builder**

```rust
#[test]
fn test_build_file_tree_flat_files() {
    let paths = vec![
        "CHANGELOG.md".to_string(),
        "README.md".to_string(),
    ];
    let tree = build_file_tree(&paths);

    assert_eq!(tree.len(), 2);
    assert_eq!(tree[0].name, "CHANGELOG.md");
    assert_eq!(tree[0].path, "CHANGELOG.md");
    assert!(!tree[0].is_dir);
    assert_eq!(tree[1].name, "README.md");
    assert_eq!(tree[1].path, "README.md");
    assert!(!tree[1].is_dir);
}

#[test]
fn test_build_file_tree_nested() {
    let paths = vec![
        "README.md".to_string(),
        "docs/api/reference.md".to_string(),
        "docs/guide.md".to_string(),
    ];
    let tree = build_file_tree(&paths);

    // Root: README.md + docs/
    assert_eq!(tree.len(), 2);
    assert_eq!(tree[0].name, "README.md");
    assert!(!tree[0].is_dir);

    let docs = &tree[1];
    assert_eq!(docs.name, "docs");
    assert!(docs.is_dir);
    assert_eq!(docs.children.len(), 2);

    // docs/ children: api/ + guide.md — directories first, then files
    let api = &docs.children[0];
    assert_eq!(api.name, "api");
    assert!(api.is_dir);
    assert_eq!(api.children.len(), 1);
    assert_eq!(api.children[0].name, "reference.md");
    assert_eq!(api.children[0].path, "docs/api/reference.md");

    let guide = &docs.children[1];
    assert_eq!(guide.name, "guide.md");
    assert_eq!(guide.path, "docs/guide.md");
    assert!(!guide.is_dir);
}

#[test]
fn test_build_file_tree_empty() {
    let paths: Vec<String> = vec![];
    let tree = build_file_tree(&paths);
    assert!(tree.is_empty());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test test_build_file_tree`
Expected: FAIL — `build_file_tree` and `TreeEntry` not found

**Step 3: Implement `TreeEntry` and `build_file_tree`**

Add near the `TrackedFile` struct:

```rust
#[derive(Debug, Clone, Serialize)]
struct TreeEntry {
    name: String,
    path: String,
    is_dir: bool,
    children: Vec<TreeEntry>,
}

fn build_file_tree(paths: &[String]) -> Vec<TreeEntry> {
    let mut root_children: Vec<TreeEntry> = Vec::new();

    for path in paths {
        let parts: Vec<&str> = path.split('/').collect();
        insert_into_tree(&mut root_children, &parts, path);
    }

    sort_tree(&mut root_children);
    root_children
}

fn insert_into_tree(entries: &mut Vec<TreeEntry>, parts: &[&str], full_path: &str) {
    if parts.is_empty() {
        return;
    }

    if parts.len() == 1 {
        // Leaf file
        entries.push(TreeEntry {
            name: parts[0].to_string(),
            path: full_path.to_string(),
            is_dir: false,
            children: Vec::new(),
        });
        return;
    }

    // Directory part — find or create
    let dir_name = parts[0];
    let existing = entries.iter_mut().find(|e| e.is_dir && e.name == dir_name);

    if let Some(dir_entry) = existing {
        insert_into_tree(&mut dir_entry.children, &parts[1..], full_path);
    } else {
        let mut new_dir = TreeEntry {
            name: dir_name.to_string(),
            path: String::new(),
            is_dir: true,
            children: Vec::new(),
        };
        insert_into_tree(&mut new_dir.children, &parts[1..], full_path);
        entries.push(new_dir);
    }
}

fn sort_tree(entries: &mut [TreeEntry]) {
    entries.sort_by(|a, b| {
        // Directories first, then files, alphabetical within each group
        match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        }
    });
    for entry in entries.iter_mut() {
        if entry.is_dir {
            sort_tree(&mut entry.children);
        }
    }
}
```

**Step 4: Run tests**

Run: `cargo test test_build_file_tree`
Expected: ALL PASS

**Step 5: Commit**

```bash
git add src/app.rs
git commit -m "feat: add tree builder for directory navigation"
```

---

### Task 6: Pass tree data to template in `render_markdown`

**Files:**
- Modify: `src/app.rs` — `render_markdown` function (lines 463-528)

**Step 1: No separate test needed** — this is wiring. The integration test from Task 8 will cover it.

**Step 2: Update `render_markdown`**

In the `if state.show_navigation()` branch, replace the flat file list building with tree building:

Replace this block:

```rust
let filenames = state.get_sorted_filenames();
let files: Vec<Value> = filenames
    .iter()
    .map(|name| {
        Value::from_object({
            let mut map = std::collections::HashMap::new();
            map.insert("name".to_string(), Value::from(name.clone()));
            map
        })
    })
    .collect();
```

with:

```rust
let filenames = state.get_sorted_filenames();
let tree = build_file_tree(&filenames);
let tree_value = Value::from_serializable(&tree);
```

And update the template context to pass `tree` instead of `files`:

```rust
match template.render(context! {
    content => content,
    mermaid_enabled => has_mermaid,
    show_navigation => true,
    tree => tree_value,
    current_file => current_file,
}) {
```

**Step 3: Run `cargo build` to check it compiles**

Run: `cargo build`
Expected: PASS (template isn't checked at compile time for variable usage)

**Step 4: Commit**

```bash
git add src/app.rs
git commit -m "feat: pass file tree structure to template"
```

---

### Task 7: Update template for collapsible directory tree

**Files:**
- Modify: `templates/main.html`

**Step 1: Replace the flat file list with recursive tree rendering**

Replace the sidebar `<ul class="file-list">` block:

```html
{% if show_navigation %}
<button class="sidebar-toggle" onclick="toggleSidebar()" aria-label="Toggle sidebar">
    <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
        <rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect>
        <line x1="9" y1="3" x2="9" y2="21"></line>
    </svg>
</button>
<nav class="sidebar">
    <div class="sidebar-header"></div>
    <div class="sidebar-content">
        <div class="recursive-toggle">
            <label class="toggle-label">
                <input type="checkbox" id="recursiveToggle" checked onchange="toggleRecursive(this.checked)">
                <span>Show all files</span>
            </label>
        </div>
        <ul class="file-list file-tree" data-depth="0">
            {% for entry in tree recursive %}
            {% if entry.is_dir %}
            <li class="tree-dir" data-depth="{{ loop.depth0 }}">
                <div class="dir-header" onclick="toggleDir(this)">
                    <span class="dir-chevron">&#9660;</span>
                    <span class="dir-name">{{ entry.name }}</span>
                </div>
                <ul class="file-list tree-children">
                    {{ loop(entry.children) }}
                </ul>
            </li>
            {% else %}
            <li class="tree-file" data-depth="{{ loop.depth0 }}">
                <a href="/{{ entry.path }}"{% if entry.path == current_file %} class="active"{% endif %}>
                    {{ entry.name }}
                </a>
            </li>
            {% endif %}
            {% endfor %}
        </ul>
    </div>
</nav>
{% endif %}
```

**Step 2: Add CSS for tree rendering**

Add inside the `{% if show_navigation %}` CSS block:

```css
/* Recursive toggle */
.recursive-toggle {
    padding: 8px 12px 12px 12px;
    border-bottom: 1px solid var(--border-color);
    margin-bottom: 8px;
}

.toggle-label {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--blockquote-color);
    cursor: pointer;
    user-select: none;
}

.toggle-label input[type="checkbox"] {
    margin: 0;
    cursor: pointer;
}

body.sidebar-collapsed .recursive-toggle {
    opacity: 0;
    pointer-events: none;
}

/* Tree structure */
.tree-dir > .tree-children {
    list-style: none;
    padding-left: 16px;
    margin: 0;
}

.tree-dir.collapsed > .tree-children {
    display: none;
}

.dir-header {
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 6px 12px;
    cursor: pointer;
    border-radius: 6px;
    font-size: 14px;
    font-weight: 600;
    color: var(--text-color);
    user-select: none;
}

.dir-header:hover {
    background: var(--border-color-light);
}

.dir-chevron {
    font-size: 10px;
    transition: transform 0.2s ease;
    display: inline-block;
    width: 14px;
    text-align: center;
}

.tree-dir.collapsed .dir-chevron {
    transform: rotate(-90deg);
}

/* Hide nested items when recursive toggle is off */
.file-tree.flat-mode > .tree-dir {
    display: none;
}
```

**Step 3: Add JavaScript for expand/collapse and recursive toggle**

Add these functions in the `<script>` block:

```javascript
// Directory tree expand/collapse
function toggleDir(headerEl) {
    const dirLi = headerEl.parentElement;
    dirLi.classList.toggle('collapsed');
    saveDirState();
}

function saveDirState() {
    const collapsed = [];
    document.querySelectorAll('.tree-dir.collapsed').forEach(el => {
        const name = el.querySelector('.dir-name').textContent;
        collapsed.push(name);
    });
    localStorage.setItem('collapsed-dirs', JSON.stringify(collapsed));
}

function restoreDirState() {
    const saved = localStorage.getItem('collapsed-dirs');
    if (!saved) return;

    try {
        const collapsed = JSON.parse(saved);
        document.querySelectorAll('.tree-dir').forEach(el => {
            const name = el.querySelector('.dir-name').textContent;
            if (collapsed.includes(name)) {
                el.classList.add('collapsed');
            }
        });
    } catch (e) {
        // ignore invalid saved state
    }
}

function expandToActive() {
    const activeLink = document.querySelector('.file-list a.active');
    if (!activeLink) return;

    let parent = activeLink.closest('.tree-dir');
    while (parent) {
        parent.classList.remove('collapsed');
        parent = parent.parentElement.closest('.tree-dir');
    }
}

// Recursive toggle (show all files vs root only)
function toggleRecursive(enabled) {
    const fileTree = document.querySelector('.file-tree');
    if (!fileTree) return;

    if (enabled) {
        fileTree.classList.remove('flat-mode');
    } else {
        fileTree.classList.add('flat-mode');
    }
    localStorage.setItem('recursive-mode', enabled ? 'true' : 'false');
}

function initRecursiveToggle() {
    const saved = localStorage.getItem('recursive-mode');
    // Default: on (recursive)
    const enabled = saved !== 'false';
    const checkbox = document.getElementById('recursiveToggle');
    if (checkbox) {
        checkbox.checked = enabled;
        toggleRecursive(enabled);
    }
}
```

Update the `DOMContentLoaded` handler to call:

```javascript
initRecursiveToggle();
restoreDirState();
expandToActive();
```

**Step 4: Rebuild (templates are embedded at compile time)**

Run: `cargo build`
Expected: PASS

**Step 5: Commit**

```bash
git add templates/main.html
git commit -m "feat: collapsible directory tree sidebar with recursive toggle"
```

---

### Task 8: Update and add integration tests

**Files:**
- Modify: `src/app.rs` — test module

**Step 1: Fix `test_directory_mode_has_navigation_sidebar`**

The test checks for `<ul class="file-list">` which still exists (as `file-list file-tree`). It should still pass, but update to also check for the tree structure class:

```rust
#[tokio::test]
async fn test_directory_mode_has_navigation_sidebar() {
    let (server, _temp_dir) = create_directory_server().await;

    let response = server.get("/test1.md").await;
    assert_eq!(response.status_code(), 200);
    let body = response.text();

    assert!(body.contains(r#"<nav class="sidebar">"#));
    assert!(body.contains(r#"class="file-list file-tree""#));
    assert!(body.contains("test1.md"));
    assert!(body.contains("test2.markdown"));
    assert!(body.contains("test3.md"));
}
```

**Step 2: Fix `test_directory_mode_active_file_highlighting`**

The `href` paths haven't changed for root-level files, so this test should still pass. Verify:

Run: `cargo test test_directory_mode_active_file_highlighting`
Expected: PASS

**Step 3: Add integration test for subdirectory tree navigation**

```rust
#[tokio::test]
async fn test_directory_mode_subdirectory_navigation() {
    let temp_dir = tempdir().expect("Failed to create temp dir");

    fs::write(temp_dir.path().join("README.md"), "# Root").expect("Failed to write");

    let docs_dir = temp_dir.path().join("docs");
    fs::create_dir(&docs_dir).expect("Failed to create docs dir");
    fs::write(docs_dir.join("guide.md"), "# Guide\n\nGuide content")
        .expect("Failed to write");

    let base_dir = temp_dir.path().to_path_buf();
    let tracked_files = scan_markdown_files(&base_dir).expect("Failed to scan");
    let router = new_router(base_dir, tracked_files, true).expect("Failed to create router");
    let server = TestServer::new(router).expect("Failed to create test server");

    // Check sidebar contains directory and file
    let response = server.get("/README.md").await;
    assert_eq!(response.status_code(), 200);
    let body = response.text();
    assert!(body.contains("docs"), "Sidebar should contain 'docs' directory");
    assert!(body.contains("guide.md"), "Sidebar should contain 'guide.md'");
    assert!(body.contains(r#"href="/docs/guide.md""#), "Should link to /docs/guide.md");

    // Serve subdirectory file
    let response = server.get("/docs/guide.md").await;
    assert_eq!(response.status_code(), 200);
    assert!(response.text().contains("Guide content"));
}
```

**Step 4: Add test for active file highlighting with subdirectory paths**

```rust
#[tokio::test]
async fn test_subdirectory_active_file_highlighting() {
    let temp_dir = tempdir().expect("Failed to create temp dir");

    fs::write(temp_dir.path().join("README.md"), "# Root").expect("Failed to write");

    let docs_dir = temp_dir.path().join("docs");
    fs::create_dir(&docs_dir).expect("Failed to create docs dir");
    fs::write(docs_dir.join("guide.md"), "# Guide").expect("Failed to write");

    let base_dir = temp_dir.path().to_path_buf();
    let tracked_files = scan_markdown_files(&base_dir).expect("Failed to scan");
    let router = new_router(base_dir, tracked_files, true).expect("Failed to create router");
    let server = TestServer::new(router).expect("Failed to create test server");

    let response = server.get("/docs/guide.md").await;
    assert_eq!(response.status_code(), 200);
    let body = response.text();
    assert!(
        body.contains(r#"href="/docs/guide.md" class="active""#),
        "Subdirectory file should be marked active"
    );
}
```

**Step 5: Run the full test suite**

Run: `cargo test`
Expected: ALL PASS

**Step 6: Run clippy and fmt**

Run: `cargo fmt && cargo clippy -- -D warnings`
Expected: PASS

**Step 7: Commit**

```bash
git add src/app.rs
git commit -m "test: add tests for recursive directory mode"
```

---

### Task 9: Update CLAUDE.md design constraints

**Files:**
- Modify: `CLAUDE.md`

**Step 1: Update the "Non-recursive" constraint**

Change:

```markdown
- **Non-recursive.** Directory mode watches only the immediate directory, never
  subdirectories. This is intentional.
```

to:

```markdown
- **Recursive with exclusions.** Directory mode recursively scans
  subdirectories for markdown files. Hidden directories (starting with `.`)
  and common build directories (`node_modules`, `target`, etc.) are excluded.
  A client-side toggle can switch to flat (root-only) view.
```

**Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: update design constraints for recursive directory mode"
```
