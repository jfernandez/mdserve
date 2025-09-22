# mdserve

Fast markdown preview server with **live reload** and **theme support**.

Just run `mdserve file.md` and start writing. One statically-compiled executable that runs anywhere - no installation, no dependencies.

## Features

- ⚡ **Instant Live Reload** - Real-time updates via WebSocket when markdown file changes
- 🎨 **Multiple Themes** - Built-in theme selector with 5 themes including Catppuccin variants
- 📝 **GitHub Flavored Markdown** - Full GFM support including tables, strikethrough, code blocks, and task lists
- 🚀 **Fast** - Built with Rust and Axum for excellent performance and low memory usage

## Installation
### From Source

```bash
git clone <repository-url>
cd mdserve
cargo install --path .
```

### Using Cargo

```bash
cargo install mdserve
```

## Usage

### Basic Usage

```bash
# Serve a markdown file on default port (3000)
mdserve README.md

# Serve on custom port
mdserve README.md --port 8080
mdserve README.md -p 8080
```


## Endpoints

Once running, the server provides (default: [http://localhost:3000](http://localhost:3000)):

- **[`/`](http://localhost:3000/)** - Rendered HTML with live reload via WebSocket
- **[`/raw`](http://localhost:3000/raw)** - Raw markdown content (useful for debugging)
- **[`/ws`](http://localhost:3000/ws)** - WebSocket endpoint for real-time updates

## Theme System

**Built-in Theme Selector**
- Click the 🎨 button in the top-right corner to open theme selector
- **5 Available Themes**:
  - **Light**: Clean, bright theme optimized for readability
  - **Dark**: GitHub-inspired dark theme with comfortable contrast
  - **Catppuccin Latte**: Warm light theme with soothing pastels
  - **Catppuccin Macchiato**: Cozy mid-tone theme with rich colors
  - **Catppuccin Mocha**: Deep dark theme with vibrant accents
- **Persistent Preference**: Your theme choice is automatically saved in browser localStorage


## Development

### Prerequisites

- Rust 1.70+ (2024 edition)
- Cargo

### Building

```bash
cargo build --release
```

### Running Tests

```bash
# Run all tests
cargo test

# Run integration tests only
cargo test --test integration_test
```




## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- Built with [Axum](https://github.com/tokio-rs/axum) web framework
- Markdown parsing by [markdown-rs](https://github.com/wooorm/markdown-rs)
- [Catppuccin](https://catppuccin.com/) color themes
- Inspired by various markdown preview tools
