use diskcopilot::cache::{self, schema, writer};
use diskcopilot::format::{format_size, parse_size};
use diskcopilot::scanner::walker::{scan_directory, ScanConfig, ScanProgress};

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::time::Instant;

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

/// Execute a scan and store results in the cache database.
fn run_scan(
    path: PathBuf,
    full: bool,
    _dirs_only: bool,
    min_size: &str,
) -> anyhow::Result<()> {
    // 1. Parse min_size
    let min_file_size = parse_size(min_size)?;

    // 2. Determine cache DB path
    let db_path = cache::db_path_for(&path)?;

    // Ensure the cache directory exists
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // 3. Remove stale cache if one exists
    if db_path.exists() {
        std::fs::remove_file(&db_path)?;
    }

    // 4. Open DB + create tables
    let mut conn = schema::open_db(&db_path)?;
    schema::create_tables(&conn)?;

    // 5. Create progress tracker and writer
    let progress = ScanProgress::new();
    let config = ScanConfig {
        min_file_size,
        cache_files: !full, // when full=true we use the full flag; otherwise respect min_size
        full,
    };

    let start = Instant::now();

    println!("Scanning: {}", path.display());

    // 6. Walk the filesystem
    {
        let mut cache_writer = writer::CacheWriter::new(&mut conn, 10_000);
        scan_directory(&path, &config, &mut cache_writer, &progress)?;

        // 7. Finalize (flush buffers + compute dir size rollups)
        cache_writer.finalize()?;

        // 9. Write scan metadata (before dropping the writer)
        let elapsed_ms = start.elapsed().as_millis() as i64;
        let meta = writer::ScanMeta {
            root_path: path.to_string_lossy().into_owned(),
            scanned_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            total_files: progress.files() as i64,
            total_dirs: progress.dirs() as i64,
            total_size: progress.size() as i64,
            scan_duration_ms: elapsed_ms,
        };
        cache_writer.write_meta(&meta)?;
        // cache_writer (and its &mut conn borrow) drops here
    }

    // 8. Create indexes after bulk insert for maximum ingestion throughput
    schema::create_indexes(&conn)?;

    let elapsed = start.elapsed();

    // 10. Print summary
    println!(
        "Done in {:.2}s — {} files, {} dirs, {} scanned",
        elapsed.as_secs_f64(),
        progress.files(),
        progress.dirs(),
        format_size(progress.size()),
    );
    println!("Cache: {}", db_path.display());

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Scan {
            path,
            full,
            dirs_only,
            min_size,
            ..
        } => run_scan(path, full, dirs_only, &min_size),
        Commands::Tui { path, .. } => {
            println!("TUI: {:?}", path.map(|p| p.display().to_string()));
            Ok(())
        }
    }
}
