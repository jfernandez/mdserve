mod app;

use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

use app::serve_markdown;

#[derive(Parser)]
#[command(name = "mdserve")]
#[command(about = "A simple HTTP server for markdown preview")]
#[command(version)]
struct Args {
    /// Path to the markdown file to serve
    file: PathBuf,

    /// Port to serve on
    #[arg(short, long, default_value = "3000")]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Canonicalize the path once for consistent absolute path display
    let absolute_path = args.file.canonicalize().unwrap_or(args.file);

    serve_markdown(absolute_path, args.port).await?;

    Ok(())
}
