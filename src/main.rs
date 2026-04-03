use diskcopilot::cache::{self, schema, writer};
use diskcopilot::cache::reader::{
    find_duplicates, load_root, load_scan_meta, load_tree_to_depth,
    query_dev_artifacts, query_large_files, query_old_files, query_recent_files,
};
use diskcopilot::delete::trash::{delete_permanent, move_to_trash};
use diskcopilot::format::{format_size, parse_size};
use diskcopilot::scanner::safety::is_dangerous_path;
use diskcopilot::scanner::walker::{scan_directory, ScanConfig, ScanProgress};
use diskcopilot::output;

use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "diskcopilot", about = "Fast Mac disk scanner and query tool")]
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

    /// Query cached scan data
    Query {
        #[command(subcommand)]
        command: QueryCommand,
    },

    /// Delete a file or directory
    Delete {
        /// Path to delete
        path: String,
        /// Move to system Trash
        #[arg(long)]
        trash: bool,
        /// Permanently delete
        #[arg(long)]
        permanent: bool,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum QueryCommand {
    /// Find largest files
    LargeFiles {
        /// Path that was scanned
        path: PathBuf,
        /// Minimum file size (e.g., 100M, 1G)
        #[arg(long, default_value = "100M")]
        min_size: String,
        /// Maximum results
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Find recently modified files
    Recent {
        path: PathBuf,
        /// Files modified within this many days
        #[arg(long, default_value = "7")]
        days: u64,
        #[arg(long, default_value = "100")]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Find old files
    Old {
        path: PathBuf,
        /// Files not modified for this many days
        #[arg(long, default_value = "365")]
        days: u64,
        #[arg(long, default_value = "100")]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Find dev artifact directories (node_modules, target, etc.)
    DevArtifacts {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Find duplicate files by content
    Duplicates {
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Show directory size tree
    Tree {
        path: PathBuf,
        /// Max depth to show
        #[arg(long, default_value = "2")]
        depth: usize,
        #[arg(long)]
        json: bool,
    },
    /// Show scan metadata
    Info {
        path: PathBuf,
        #[arg(long)]
        json: bool,
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
    if is_dangerous_path(&path) {
        anyhow::bail!(
            "Refusing to scan '{}': system-protected path. Use a more specific subdirectory.",
            path.display()
        );
    }
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

    // 4. Open DB + create tables (bulk-write mode for scan)
    let mut conn = schema::open_db_for_scan(&db_path)?;
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

fn run_query(command: QueryCommand) -> anyhow::Result<()> {
    // Helper to open DB for a given path
    let open_cache = |path: &Path| -> anyhow::Result<rusqlite::Connection> {
        let db_path = cache::db_path_for(path)?;
        if !db_path.exists() {
            anyhow::bail!(
                "No cache found for '{}'. Run 'diskcopilot scan {}' first.",
                path.display(),
                path.display()
            );
        }
        schema::open_db(&db_path)
    };

    match command {
        QueryCommand::LargeFiles {
            path,
            min_size,
            limit,
            json,
        } => {
            let conn = open_cache(&path)?;
            let min = parse_size(&min_size)?;
            let rows = query_large_files(&conn, min, limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                output::print_file_rows(&rows);
            }
        }
        QueryCommand::Recent {
            path,
            days,
            limit,
            json,
        } => {
            let conn = open_cache(&path)?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let cutoff = now - (days as i64 * 86400);
            let rows = query_recent_files(&conn, cutoff, limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                output::print_file_rows(&rows);
            }
        }
        QueryCommand::Old {
            path,
            days,
            limit,
            json,
        } => {
            let conn = open_cache(&path)?;
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let cutoff = now - (days as i64 * 86400);
            let rows = query_old_files(&conn, cutoff, limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                output::print_file_rows(&rows);
            }
        }
        QueryCommand::DevArtifacts { path, json } => {
            let conn = open_cache(&path)?;
            let nodes = query_dev_artifacts(&conn)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&nodes)?);
            } else {
                output::print_tree_nodes(&nodes);
            }
        }
        QueryCommand::Duplicates { path, json } => {
            let conn = open_cache(&path)?;
            let groups = find_duplicates(&conn, |done, total| {
                if !json {
                    eprint!("\r  Hashing... {}/{}", done, total);
                }
            })?;
            if !json {
                eprintln!(); // clear progress line
            }
            if json {
                println!("{}", serde_json::to_string_pretty(&groups)?);
            } else {
                output::print_duplicate_groups(&groups);
            }
        }
        QueryCommand::Tree { path, depth, json } => {
            let conn = open_cache(&path)?;
            let root = load_root(&conn)?;
            let tree = load_tree_to_depth(&conn, root.id, depth)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&tree)?);
            } else {
                output::print_tree(&tree, 0);
            }
        }
        QueryCommand::Info { path, json } => {
            let conn = open_cache(&path)?;
            let meta = load_scan_meta(&conn)?;
            match meta {
                Some(m) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&m)?);
                    } else {
                        output::print_scan_meta(&m);
                    }
                }
                None => {
                    if json {
                        println!("null");
                    } else {
                        println!("  No scan metadata found.");
                    }
                }
            }
        }
    }
    Ok(())
}

fn run_delete(path: String, trash: bool, permanent: bool, json: bool) -> anyhow::Result<()> {
    if !trash && !permanent {
        anyhow::bail!("Specify --trash or --permanent");
    }
    if trash && permanent {
        anyhow::bail!("Specify either --trash or --permanent, not both");
    }

    let result = if trash {
        move_to_trash(&path)?
    } else {
        delete_permanent(&path)?
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if result.success {
        println!(
            "  Deleted: {} (freed {})",
            result.path,
            format_size(result.size_freed)
        );
    } else {
        eprintln!(
            "  Error: {}",
            result.error.as_deref().unwrap_or("unknown error")
        );
        std::process::exit(1);
    }
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
        Commands::Query { command } => run_query(command),
        Commands::Delete {
            path,
            trash,
            permanent,
            json,
        } => run_delete(path, trash, permanent, json),
    }
}
