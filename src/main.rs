use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

mod app;

use app::{scan_markdown_files, serve_markdown, RtlMode};

#[derive(Parser)]
#[command(name = "mdserve")]
#[command(about = "A simple HTTP server for markdown preview")]
#[command(version)]
struct Args {
    /// Path to markdown file or directory to serve
    path: PathBuf,

    /// Hostname (domain or IP address) to listen on
    #[arg(short = 'H', long, default_value = "127.0.0.1")]
    hostname: String,

    /// Port to serve on
    #[arg(short, long, default_value = "3000")]
    port: u16,

    /// Open the preview in the default browser
    #[arg(short, long)]
    open: bool,

    /// Force RTL rendering for all documents
    #[arg(long)]
    rtl: bool,

    /// Disable RTL auto-detection; force LTR for all documents
    #[arg(long, conflicts_with = "rtl")]
    no_rtl: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let absolute_path = args.path.canonicalize().unwrap_or(args.path);

    let (base_dir, tracked_files, is_directory_mode) = if absolute_path.is_file() {
        // Single-file mode: derive parent directory
        let base_dir = absolute_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."))
            .to_path_buf();
        let tracked_files = vec![absolute_path];
        (base_dir, tracked_files, false)
    } else if absolute_path.is_dir() {
        // Directory mode: scan directory for markdown files
        let tracked_files = scan_markdown_files(&absolute_path)?;
        if tracked_files.is_empty() {
            anyhow::bail!("No markdown files found in directory");
        }
        (absolute_path, tracked_files, true)
    } else {
        anyhow::bail!("Path must be a file or directory");
    };

    // Determine RTL mode from flags
    let rtl_mode = if args.rtl {
        RtlMode::Force
    } else if args.no_rtl {
        RtlMode::Disabled
    } else {
        RtlMode::Auto
    };

    // Single unified serve function
    serve_markdown(
        base_dir,
        tracked_files,
        is_directory_mode,
        args.hostname,
        args.port,
        args.open,
        rtl_mode,
    )
    .await?;

    Ok(())
}
