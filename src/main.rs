use diskcopilot::cache::{self, schema, writer};
use diskcopilot::cache::reader::{
    find_duplicates, load_root, load_scan_meta, load_tree_to_depth,
    query_by_extension, query_by_name, query_dev_artifacts,
    query_large_files, query_old_files, query_recent_files, query_summary,
};
use diskcopilot::cache::writer::ScanMeta;
use diskcopilot::delete::trash::{delete_permanent, move_to_trash};
use diskcopilot::format::{format_size, parse_size};
use diskcopilot::scanner::safety::is_dangerous_path;
use diskcopilot::scanner::walker::{scan_directory, ScanConfig, ScanProgress};
#[cfg(target_os = "macos")]
use diskcopilot::scanner::bulk_walker::{scan_directory_bulk, supports_bulk_attrs};
use diskcopilot::output;
use diskcopilot::safelist;
use diskcopilot::server;

use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
#[allow(unused_imports)]
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Parser)]
#[command(
    name = "diskcopilot-cli",
    version,
    about = "Fast Mac disk scanner and query tool",
    after_help = "\x1b[1mExamples:\x1b[0m
  diskcopilot-cli scan ~                           Scan home directory
  diskcopilot-cli scan ~ --full                    Scan all files (no size threshold)
  diskcopilot-cli query tree ~ --depth 2           Directory size tree
  diskcopilot-cli query large-files ~ --json       Largest files as JSON
  diskcopilot-cli query dev-artifacts ~            Find node_modules, target, etc.
  diskcopilot-cli query sql \"SELECT ...\" ~         Raw SQL query
  diskcopilot-cli serve ~                          Open cleanup dashboard in browser
  diskcopilot-cli delete ~/old-file.zip --trash    Move to Trash"
)]
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

    /// Mark a file or folder as important (excluded from cleanup recommendations)
    Keep {
        /// Path to protect
        path: PathBuf,
    },

    /// Remove a file or folder from the safelist
    Unkeep {
        /// Path to unprotect
        path: PathBuf,
    },

    /// Show all safelist entries
    KeepList,

    /// Start interactive cleanup web UI
    Serve {
        /// Path that was scanned
        path: PathBuf,
        /// Port to listen on
        #[arg(long, default_value = "3847")]
        port: u16,
        /// AI-generated insights text to display
        #[arg(long)]
        insights: Option<String>,
        /// Path to a file containing AI insights
        #[arg(long)]
        insights_file: Option<PathBuf>,
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
    /// Run a raw SQL query against the cache database
    Sql {
        /// SQL query to execute (read-only)
        query: String,
        /// Path that was scanned
        path: PathBuf,
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
    if is_dangerous_path(&path) && !force {
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
    conn.execute_batch("PRAGMA foreign_keys=OFF; PRAGMA locking_mode=EXCLUSIVE;")?;
    {
        let mut cache_writer = writer::CacheWriter::new(&mut conn, 500_000);
        cache_writer.begin()?;

        // On macOS, prefer the getattrlistbulk-based scanner (3-6x faster on APFS).
        // Fall back to the jwalk-based scanner for non-APFS volumes (exFAT, FAT32, NTFS).
        #[cfg(target_os = "macos")]
        let skipped_sizes = if supports_bulk_attrs(&path) {
            scan_directory_bulk(&path, &config, &mut cache_writer, &progress)?
        } else {
            scan_directory(&path, &config, &mut cache_writer, &progress)?
        };

        #[cfg(not(target_os = "macos"))]
        let skipped_sizes = {
            scan_directory(&path, &config, &mut cache_writer, &progress)?
        };

        cache_writer.commit()?;

        // 7. Finalize (flush buffers + compute dir size rollups)
        cache_writer.finalize(&skipped_sizes)?;
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
    {
        let mut meta_writer = writer::CacheWriter::new(&mut conn, 1);
        meta_writer.write_meta(&ScanMeta {
            root_path: path.to_string_lossy().into_owned(),
            scanned_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
            total_files,
            total_dirs,
            total_size,
            scan_duration_ms: elapsed.as_millis() as i64,
        })?;
    }

    let files = total_files as u64;
    let dirs = total_dirs as u64;
    let size = format_size(total_size as u64);
    let secs = elapsed.as_secs_f64();
    let rate = if secs > 0.0 { files as f64 / secs } else { 0.0 };
    let db_size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);

    println!("\n\x1b[1;32m✓ Scan complete\x1b[0m");
    if full {
        println!("  \x1b[1mMode:\x1b[0m    Full (all files)");
    } else {
        println!("  \x1b[1mMode:\x1b[0m    Default (files >= {}) \x1b[90m— directory sizes are accurate, use --full for small file queries\x1b[0m", format_size(min_file_size));
    }
    println!("  \x1b[1mFiles:\x1b[0m   \x1b[33m{}\x1b[0m", files);
    println!("  \x1b[1mDirs:\x1b[0m    \x1b[33m{}\x1b[0m", dirs);
    println!("  \x1b[1mSize:\x1b[0m    \x1b[32m{}\x1b[0m", size);
    println!("  \x1b[1mTime:\x1b[0m    \x1b[36m{:.2}s\x1b[0m ({:.0} files/s)", secs, rate);
    println!("  \x1b[1mCache:\x1b[0m   {} \x1b[90m({})\x1b[0m", format_size(db_size), db_path.display());

    // Print rich post-scan report
    output::print_scan_report(&conn);

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
        QueryCommand::Sql { query, path } => {
            let conn = open_cache(&path)?;
            // Only allow read-only queries
            let q = query.trim().to_uppercase();
            if !q.starts_with("SELECT") && !q.starts_with("WITH") {
                anyhow::bail!("Only SELECT and WITH statements are allowed");
            }
            let mut stmt = conn.prepare(&query)?;
            let col_count = stmt.column_count();
            let col_names: Vec<String> = (0..col_count)
                .map(|i| stmt.column_name(i).unwrap_or("?").to_string())
                .collect();

            let rows = stmt.query_map([], |row| {
                let mut map = serde_json::Map::new();
                for (i, col_name) in col_names.iter().enumerate() {
                    let val: rusqlite::types::Value = row.get(i)?;
                    let json_val = match val {
                        rusqlite::types::Value::Null => serde_json::Value::Null,
                        rusqlite::types::Value::Integer(n) => serde_json::json!(n),
                        rusqlite::types::Value::Real(f) => serde_json::json!(f),
                        rusqlite::types::Value::Text(s) => serde_json::json!(s),
                        rusqlite::types::Value::Blob(b) => serde_json::json!(format!("<blob {} bytes>", b.len())),
                    };
                    map.insert(col_name.clone(), json_val);
                }
                Ok(serde_json::Value::Object(map))
            })?;

            let result: Vec<serde_json::Value> = rows
                .collect::<rusqlite::Result<Vec<_>>>()?;
            println!("{}", serde_json::to_string_pretty(&result)?);
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
        Commands::Serve {
            path,
            port,
            insights,
            insights_file,
        } => {
            let insights_content = if let Some(file) = insights_file {
                Some(std::fs::read_to_string(file)?)
            } else {
                insights
            };
            server::serve(path, port, insights_content).await
        }
        Commands::Keep { path } => {
            safelist::add(&path)?;
            println!("  Protected: {}", path.display());
            Ok(())
        }
        Commands::Unkeep { path } => {
            safelist::remove(&path)?;
            println!("  Removed from safelist: {}", path.display());
            Ok(())
        }
        Commands::KeepList => {
            let entries = safelist::load()?;
            if entries.is_empty() {
                println!("  Safelist is empty. Use 'diskcopilot-cli keep <path>' to protect files.");
            } else {
                println!("  Protected files/folders:");
                let mut sorted: Vec<_> = entries.iter().collect();
                sorted.sort();
                for p in sorted {
                    println!("    {}", p.display());
                }
            }
            Ok(())
        }
    }
}
