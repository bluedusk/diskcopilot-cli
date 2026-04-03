use diskcopilot::cache::{self, schema, writer};
use diskcopilot::format::{format_size, parse_size};
use diskcopilot::scanner::walker::{scan_directory, ScanConfig, ScanProgress};

use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
use std::sync::Arc;
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
async fn run_scan(
    path: PathBuf,
    full: bool,
    dirs_only: bool,
    accurate: bool,
    cross_firmlinks: bool,
    min_size: &str,
) -> anyhow::Result<()> {
    if dirs_only {
        eprintln!("Warning: --dirs-only is not yet implemented");
    }
    if accurate {
        eprintln!("Warning: --accurate is not yet implemented");
    }
    if cross_firmlinks {
        eprintln!("Warning: --cross-firmlinks is not yet implemented");
    }
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
    let progress = Arc::new(ScanProgress::new());
    let config = ScanConfig {
        min_file_size,
        full,
    };

    let start = Instant::now();

    // Set up spinner
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .expect("valid spinner template"),
    );
    pb.set_message(format!("Scanning {}...", path.display()));

    // Spawn a task that updates the spinner message every 100ms
    let pb_clone = pb.clone();
    let progress_clone = Arc::clone(&progress);
    let spinner_handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            ticker.tick().await;
            let files = progress_clone.files();
            let dirs = progress_clone.dirs();
            let size = format_size(progress_clone.size());
            pb_clone.set_message(format!("Scanning... {} files, {} dirs, {}", files, dirs, size));
        }
    });

    // 6. Walk the filesystem
    // Disable FK checks during scan to allow inserting entries in any order
    // (jwalk's parallel walk does not guarantee parent-before-child ordering).
    conn.execute_batch("PRAGMA foreign_keys=OFF;")?;
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
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;

    // Stop spinner
    spinner_handle.abort();
    pb.finish_and_clear();

    // 8. Create indexes after bulk insert for maximum ingestion throughput
    schema::create_indexes(&conn)?;

    let elapsed = start.elapsed();

    // 10. Print summary
    println!(
        "✓ {} files, {} dirs, {} scanned in {:.2}s",
        progress.files(),
        progress.dirs(),
        format_size(progress.size()),
        elapsed.as_secs_f64(),
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
            accurate,
            cross_firmlinks,
        } => run_scan(path, full, dirs_only, accurate, cross_firmlinks, &min_size).await,
        Commands::Tui { path, theme, cached, depth, top, .. } => {
            if cached {
                eprintln!("Warning: --cached is not yet implemented");
            }
            if depth.is_some() {
                eprintln!("Warning: --depth is not yet implemented");
            }
            if top.is_some() {
                eprintln!("Warning: --top is not yet implemented");
            }
            // Determine path and look for cache
            let target = path.unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let db_path = diskcopilot::cache::db_path_for(&target)?;

            if !db_path.exists() {
                eprintln!(
                    "No cache found for {}. Run `diskcopilot scan {}` first.",
                    target.display(),
                    target.display()
                );
                std::process::exit(1);
            }

            let conn = diskcopilot::cache::schema::open_db(&db_path)?;
            diskcopilot::tui::app::run(conn, &theme).await
        }
    }
}
