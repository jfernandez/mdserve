---
name: mdserve
description: >-
  Preview markdown with mdserve when content is long or likely to be
  iterated with the user (tables, diagrams, multi-section docs). Skip
  preview for short markdown that is easy to read directly in the
  terminal.
---

# mdserve

Serve markdown files as live-reloading HTML previews in the browser
using `mdserve`.

## When to use

Use mdserve whenever you produce markdown that benefits from rendered
presentation:

- Plans and proposals
- Architecture or design documents
- Reports, comparisons, or summaries with tables
- Anything containing Mermaid diagrams
- Multi-file documentation sets
- Any time the user asks to "preview" or "render" markdown

Use mdserve when markdown is more than about 40 to 60 lines, has
complex formatting, or is likely to go through multiple edit/review
iterations with the user.

Do **not** use mdserve for short conversational answers, single code
snippets, trivial one-paragraph responses, or any markdown that fits
comfortably within a terminal window.

## Workflow

1. Write the markdown file (e.g. `plan.md`).
2. Start mdserve in the background with `--open` to launch the browser
   automatically:
   ```bash
   mdserve --open plan.md &
   ```
3. Tell the user the URL (default: http://127.0.0.1:3000).
4. Continue editing the file - changes reload automatically.
5. When the task is finished and the preview is no longer needed, kill
   the background process.

## Port conflicts

Before starting mdserve, check if the default port is in use:

```bash
ss -tlnp | grep :3000
```

If port 3000 is occupied, pick another port:

```bash
mdserve --open plan.md --port 3001 &
```

Always tell the user the actual URL including the port you used.

## Directory mode

When producing multiple related markdown files, serve the parent
directory instead:

```bash
mdserve --open docs/ &
```

This gives the user a sidebar to navigate between files. Only the
immediate directory is watched (non-recursive).

## Mermaid diagrams

Use Mermaid diagrams when they improve clarity over plain text:

- **Flowcharts** — processes and decision trees
- **Sequence diagrams** — API and service interactions
- **Entity-relationship diagrams** — data models
- **State diagrams** — state machines

Prefer Mermaid over ASCII art when the diagram has more than a few
elements or shows relationships and flow.

## Installation

mdserve must be installed on the user's system. If the `mdserve`
command is not found, ask the user how they would like to install it
using `AskUserQuestion` with these options:

1. **Install script** — `curl -sSfL https://raw.githubusercontent.com/jfernandez/mdserve/main/install.sh | bash`
2. **Homebrew** — `brew install mdserve`
3. **Cargo** — `cargo install mdserve`
4. **Arch Linux** — `sudo pacman -S mdserve`

Then run the corresponding install command for them.
