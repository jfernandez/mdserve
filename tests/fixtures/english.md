# Installation and Configuration Guide

## Introduction

This document describes the installation and configuration process for mdserve, a Markdown preview server designed to work alongside intelligent code agents.

## Prerequisites

- Rust 1.82 or later
- A supported operating system (macOS, Linux, or Windows)
- Command-line interface or Terminal

## Installation

### Method One: Build from Source

```bash
git clone https://github.com/anthropic/mdserve.git
cd mdserve
cargo build --release
./target/release/mdserve /path/to/file.md
```

### Method Two: Install with Cargo

```bash
cargo install --path .
mdserve /path/to/file.md
```

## Basic Usage

### View a Single File

```bash
mdserve document.md
```

### View a Directory

```bash
mdserve /path/to/docs/
```

## Command-Line Options

| Option | Description |
|--------|-------------|
| `-H, --hostname` | IP address or hostname (default: 127.0.0.1) |
| `-p, --port` | Port to listen on (default: 3000) |
| `-o, --open` | Open in system browser |
| `--rtl` | Force RTL for all documents |
| `--no-rtl` | Disable automatic RTL detection |

## Features

- **Automatic RTL Detection**: Detects Hebrew, Arabic, Farsi, and other RTL languages
- **Live Reload**: Automatic refresh when files change
- **Themes**: Light, dark, and Catppuccin Latte/Macchiato/Mocha
- **Mermaid Support**: Interactive diagrams and flowcharts
- **Zero Configuration**: Simply run mdserve with a file or directory

## Examples

> mdserve uses a "zero configuration" approach - no configuration files needed. Just run the command and find the interface in your browser.

### Example with RTL Forced

```bash
mdserve --rtl document.md
```

### Example with LTR Forced

```bash
mdserve --no-rtl document.md
```

## Common Errors

- **Error**: "No markdown files found in directory"
  - **Solution**: Ensure the directory contains `.md` or `.markdown` files

- **Error**: "Port already in use"
  - **Solution**: mdserve will automatically search for an available port

## Support

For questions and issues, please visit [GitHub Issues](https://github.com/anthropic/mdserve/issues)
