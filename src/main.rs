mod format;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "diskcopilot", about = "Fast Mac disk scanner with interactive TUI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan filesystem and cache results
    Scan {
        /// Path to scan
        path: PathBuf,

        /// Cache every file, no size threshold
        #[arg(long)]
        full: bool,

        /// Cache directory aggregates only
        #[arg(long)]
        dirs_only: bool,

        /// Minimum file size to cache (e.g., 10M, 1G)
        #[arg(long, default_value = "1M")]
        min_size: String,

        /// Enable APFS clone detection + xattr measurement
        #[arg(long)]
        accurate: bool,

        /// Follow firmlinks (macOS system volumes)
        #[arg(long)]
        cross_firmlinks: bool,
    },

    /// Launch interactive TUI
    Tui {
        /// Path to browse (scans if no cache exists)
        path: Option<PathBuf>,

        /// Use cached scan data only (no re-scan)
        #[arg(long)]
        cached: bool,

        /// Limit tree display depth
        #[arg(long)]
        depth: Option<usize>,

        /// Show only N largest entries per directory
        #[arg(long)]
        top: Option<usize>,

        /// Theme name
        #[arg(long, default_value = "dark")]
        theme: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Scan { path, .. } => {
            println!("Scanning: {}", path.display());
            Ok(())
        }
        Commands::Tui { path, .. } => {
            println!("TUI: {:?}", path.map(|p| p.display().to_string()));
            Ok(())
        }
    }
}
