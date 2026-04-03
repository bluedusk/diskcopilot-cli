use diskcopilot::cache::{self, schema, writer};
use diskcopilot::cache::reader::{
    find_duplicates, load_root, load_scan_meta, load_tree_to_depth,
    query_by_extension, query_by_name, query_dev_artifacts, query_large_files,
    query_old_files, query_recent_files, query_summary,
};
use diskcopilot::delete::trash::{delete_permanent, move_to_trash};
use diskcopilot::format::{format_size, parse_size};
use diskcopilot::scanner::walker::{scan_directory, ScanConfig, ScanProgress};
#[cfg(target_os = "macos")]
use diskcopilot::scanner::bulk_walker::{scan_directory_bulk, supports_bulk_attrs};
use diskcopilot::output;

use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Parser)]
#[command(name = "diskcopilot-cli", about = "Fast Mac disk scanner and query tool")]
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

        /// Skip safety warnings for system paths
        #[arg(long)]
        force: bool,
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
    /// Find files by extension
    Ext {
        path: PathBuf,
        /// File extension (e.g., dmg, pdf, mp4)
        #[arg(long)]
        ext: String,
        #[arg(long, default_value = "50")]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Search files by name
    Search {
        path: PathBuf,
        /// Search pattern (case-insensitive substring)
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "50")]
        limit: usize,
        #[arg(long)]
        json: bool,
    },
    /// Cleanup summary report
    Summary {
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
    force: bool,
    min_size: &str,
) -> anyhow::Result<()> {
    // Warn on system paths, but let --force override
    let path_str = path.to_string_lossy();
    let is_system = path_str == "/"
        || path_str.starts_with("/System")
        || path_str.starts_with("/Library")
        || path_str.starts_with("/private");
    if is_system && !force {
        anyhow::bail!(
            "'{}' is a system path. This may take a very long time. Use --force to proceed.",
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

    // Set up colorful progress bar
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .expect("valid spinner template"),
    );
    pb.enable_steady_tick(std::time::Duration::from_millis(80));

    // Spawn a task that updates the spinner with elapsed time
    let pb_clone = pb.clone();
    let path_display = path.display().to_string();
    let spinner_handle = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(std::time::Duration::from_millis(100));
        loop {
            ticker.tick().await;
            let elapsed = start.elapsed().as_secs();
            pb_clone.set_message(format!(
                "\x1b[1mScanning\x1b[0m \x1b[36m{}\x1b[0m \x1b[90m({}s)\x1b[0m",
                path_display, elapsed
            ));
        }
    });

    // 6. Walk the filesystem
    // Disable FK checks during scan to allow inserting entries in any order
    // (jwalk's parallel walk does not guarantee parent-before-child ordering).
    conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA journal_mode=OFF; PRAGMA locking_mode=EXCLUSIVE;")?;
    {
        let mut cache_writer = writer::CacheWriter::new(&mut conn, 500_000);
        cache_writer.begin()?;

        // On macOS, prefer the getattrlistbulk-based scanner (3-6x faster on APFS).
        // Fall back to the jwalk-based scanner for non-APFS volumes (exFAT, FAT32, NTFS).
        #[cfg(target_os = "macos")]
        let used_bulk = if supports_bulk_attrs(&path) {
            scan_directory_bulk(&path, &config, &mut cache_writer, &progress)?;
            true
        } else {
            scan_directory(&path, &config, &mut cache_writer, &progress)?;
            false
        };

        #[cfg(not(target_os = "macos"))]
        let used_bulk = {
            scan_directory(&path, &config, &mut cache_writer, &progress)?;
            false
        };

        let _ = used_bulk; // suppress unused-variable warning in some cfg combinations

        cache_writer.commit()?;

        // 7. Finalize (flush buffers + compute dir size rollups)
        cache_writer.finalize()?;
        // cache_writer (and its &mut conn borrow) drops here
    }
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;

    // Stop spinner
    spinner_handle.abort();
    pb.set_message("Building indexes...");

    // 8. Create indexes after bulk insert for maximum ingestion throughput
    schema::create_indexes(&conn)?;

    pb.finish_and_clear();
    let elapsed = start.elapsed();

    // Get accurate totals from DB (includes bundles and rollup)
    let (total_files, total_dirs, total_size) = conn.query_row(
        "SELECT COALESCE(total_file_count,0),
                (SELECT COUNT(*) FROM dirs),
                COALESCE(total_disk_size,0)
         FROM dirs WHERE parent_id IS NULL",
        [],
        |r| Ok((r.get::<_,i64>(0)?, r.get::<_,i64>(1)?, r.get::<_,i64>(2)?)),
    ).unwrap_or((progress.files() as i64, progress.dirs() as i64, progress.size() as i64));

    // Write scan metadata with accurate DB totals
    conn.execute(
        "INSERT INTO scan_meta (root_path, scanned_at, total_files, total_dirs, total_size, scan_duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        rusqlite::params![
            path.to_string_lossy().as_ref(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            total_files,
            total_dirs,
            total_size,
            elapsed.as_millis() as i64,
        ],
    )?;

    let files = total_files as u64;
    let dirs = total_dirs as u64;
    let size = format_size(total_size as u64);
    let secs = elapsed.as_secs_f64();
    let rate = if secs > 0.0 { files as f64 / secs } else { 0.0 };
    let db_size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

    println!("\n\x1b[1;32m✓ Scan complete\x1b[0m");
    println!("  \x1b[1mFiles:\x1b[0m   \x1b[33m{}\x1b[0m", files);
    println!("  \x1b[1mDirs:\x1b[0m    \x1b[33m{}\x1b[0m", dirs);
    println!("  \x1b[1mSize:\x1b[0m    \x1b[32m{}\x1b[0m", size);
    println!("  \x1b[1mTime:\x1b[0m    \x1b[36m{:.2}s\x1b[0m ({:.0} files/s)", secs, rate);
    println!("  \x1b[1mCache:\x1b[0m   {} \x1b[90m({})\x1b[0m", format_size(db_size), db_path.display());

    // Show disk context for root/drive scans
    #[cfg(target_os = "macos")]
    {
        use std::ffi::CString;
        use std::mem::MaybeUninit;
        if let Ok(c_path) = CString::new(path.as_os_str().as_encoded_bytes()) {
            let mut stat: MaybeUninit<libc::statfs> = MaybeUninit::uninit();
            if unsafe { libc::statfs(c_path.as_ptr(), stat.as_mut_ptr()) } == 0 {
                let stat = unsafe { stat.assume_init() };
                let block_size = stat.f_bsize as u64;
                let total_space = stat.f_blocks * block_size;
                let free_space = stat.f_bavail * block_size;
                let used_space = total_space - free_space;
                let scanned = total_size as u64;
                if used_space > scanned {
                    let hidden = used_space - scanned;
                    println!("  \x1b[1mDisk:\x1b[0m    {} total, {} free", format_size(total_space), format_size(free_space));
                    println!("  \x1b[1mHidden:\x1b[0m  \x1b[35m{}\x1b[0m \x1b[90m(snapshots, protected system files, APFS metadata)\x1b[0m", format_size(hidden));
                }
            }
        }
    }

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
        QueryCommand::Ext {
            path,
            ext,
            limit,
            json,
        } => {
            let conn = open_cache(&path)?;
            let rows = query_by_extension(&conn, &ext, limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                output::print_file_rows(&rows);
            }
        }
        QueryCommand::Search {
            path,
            name,
            limit,
            json,
        } => {
            let conn = open_cache(&path)?;
            let rows = query_by_name(&conn, &name, limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                output::print_file_rows(&rows);
            }
        }
        QueryCommand::Summary { path, json } => {
            let conn = open_cache(&path)?;
            let summary = query_summary(&conn)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&summary)?);
            } else {
                output::print_summary(&summary);
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
            force,
        } => run_scan(path, full, dirs_only, accurate, cross_firmlinks, force, &min_size).await,
        Commands::Query { command } => run_query(command),
        Commands::Delete {
            path,
            trash,
            permanent,
            json,
        } => run_delete(path, trash, permanent, json),
    }
}
