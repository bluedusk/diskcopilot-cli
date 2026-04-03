# DiskCopilot CLI v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a fast, accurate Mac disk scanner with a polished interactive TUI for browsing and cleaning up disk space.

**Architecture:** Scan-first pipeline — parallel filesystem walk (jwalk) collects metadata, stores in normalized SQLite cache, then TUI reads from cache for instant navigation. Yazi-inspired async event loop with debounced rendering. Six switchable views (Tree, Large Files, Recent, Old, Dev Artifacts, Duplicates).

**Tech Stack:** Rust, clap (CLI), jwalk (parallel walk), rusqlite (SQLite), ratatui + crossterm (TUI), tui-tree-widget, nucleo (fuzzy search), tokio (async runtime)

**Spec:** `docs/superpowers/specs/2026-04-03-diskcopilot-design.md`

---

## File Map

### Files to Create

```
Cargo.toml                          # workspace root with all dependencies
src/main.rs                         # CLI entry point, clap subcommands, tokio runtime
src/scanner/mod.rs                  # re-exports
src/scanner/walker.rs               # jwalk parallel traversal, inode dedup, symlink/firmlink handling
src/scanner/metadata.rs             # macOS file metadata extraction (lstat, blocks, timestamps)
src/scanner/safety.rs               # dangerous path blocklist for delete protection
src/cache/mod.rs                    # re-exports, connection management
src/cache/schema.rs                 # SQLite table definitions, PRAGMA setup, migrations
src/cache/writer.rs                 # batched bulk inserts during scan
src/cache/reader.rs                 # query execution, tree reconstruction, view queries
src/config/mod.rs                   # re-exports
src/config/loader.rs                # TOML config loading, defaults, theme selection
src/tui/mod.rs                      # re-exports
src/tui/app.rs                      # App state, async event loop, key handling, render dispatch
src/tui/event.rs                    # Event enum, global mpsc channel, render flag
src/tui/tree.rs                     # Tree widget with size/icon columns and percentage bars
src/tui/views.rs                    # View enum, view-specific data loading from cache
src/tui/detail.rs                   # Detail pane widget (right panel)
src/tui/search.rs                   # Fuzzy search overlay using nucleo
src/tui/theme.rs                    # Theme struct, built-in themes, color definitions
src/tui/icons.rs                    # Nerd Font icon mapping by extension/type
src/tui/statusbar.rs                # Bottom status bar widget
src/tui/tabs.rs                     # Top tab bar widget for view switching
src/delete/mod.rs                   # re-exports
src/delete/trash.rs                 # macOS Trash deletion, dry-run, safety checks
src/format.rs                       # Human-readable size formatting, date formatting
themes/dark.toml                    # dark theme definition
tests/scanner_test.rs               # integration tests for scanner
tests/cache_test.rs                 # integration tests for cache read/write
```

---

## Task 1: Project Scaffold

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `src/format.rs`

- [ ] **Step 1: Initialize Cargo project**

```bash
cd /Users/danzhu/playground/diskcopilot-cli
cargo init --name diskcopilot
```

- [ ] **Step 2: Replace Cargo.toml with full dependencies**

Replace `Cargo.toml`:

```toml
[package]
name = "diskcopilot"
version = "0.1.0"
edition = "2021"
description = "Fast, accurate Mac disk scanner with interactive TUI"

[dependencies]
# CLI
clap = { version = "4", features = ["derive"] }

# Async
tokio = { version = "1", features = ["full"] }

# Scanning
jwalk = "0.8"

# Cache
rusqlite = { version = "0.31", features = ["bundled"] }

# TUI
ratatui = "0.28"
crossterm = "0.28"
tui-tree-widget = "0.22"

# Search
nucleo = "0.5"

# Serialization & config
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# Utils
dirs = "5"
indicatif = "0.17"
blake3 = "1"
trash = "5"
anyhow = "1"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: Write format.rs with size formatting**

Create `src/format.rs`:

```rust
/// Format bytes into human-readable string (e.g., 1.2 GB, 340 MB, 4.5 KB).
pub fn format_size(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

/// Parse a size string like "100M", "1G", "500K" into bytes.
pub fn parse_size(s: &str) -> anyhow::Result<u64> {
    let s = s.trim();
    let (num, unit) = if s.ends_with(|c: char| c.is_alphabetic()) {
        let split = s.len() - 1;
        (&s[..split], &s[split..])
    } else {
        (s, "")
    };

    let value: f64 = num.parse().map_err(|_| anyhow::anyhow!("invalid size: {}", s))?;

    let multiplier: u64 = match unit.to_uppercase().as_str() {
        "K" | "KB" => 1024,
        "M" | "MB" => 1024 * 1024,
        "G" | "GB" => 1024 * 1024 * 1024,
        "T" | "TB" => 1024 * 1024 * 1024 * 1024,
        "" | "B" => 1,
        _ => return Err(anyhow::anyhow!("unknown size unit: {}", unit)),
    };

    Ok((value * multiplier as f64) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1024), "1.0 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(1048576), "1.0 MB");
        assert_eq!(format_size(1073741824), "1.0 GB");
        assert_eq!(format_size(1099511627776), "1.0 TB");
    }

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("100M").unwrap(), 104857600);
        assert_eq!(parse_size("1G").unwrap(), 1073741824);
        assert_eq!(parse_size("500K").unwrap(), 512000);
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert!(parse_size("abc").is_err());
    }
}
```

- [ ] **Step 4: Write minimal main.rs with clap skeleton**

Create `src/main.rs`:

```rust
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
```

- [ ] **Step 5: Verify it builds and tests pass**

Run: `cargo test`
Expected: All format tests pass.

Run: `cargo run -- scan /tmp`
Expected: Prints "Scanning: /tmp"

Run: `cargo run -- tui /tmp`
Expected: Prints "TUI: Some("/tmp")"

- [ ] **Step 6: Commit**

```bash
git init
echo "target/" > .gitignore
git add Cargo.toml src/main.rs src/format.rs .gitignore
git commit -m "feat: project scaffold with CLI skeleton and size formatting"
```

---

## Task 2: SQLite Cache Schema

**Files:**
- Create: `src/cache/mod.rs`
- Create: `src/cache/schema.rs`

- [ ] **Step 1: Write cache schema tests**

Create `src/cache/schema.rs`:

```rust
use anyhow::Result;
use rusqlite::Connection;

/// Open or create a SQLite database at the given path with optimized PRAGMAs.
pub fn open_db(path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=OFF;
         PRAGMA foreign_keys=ON;
         PRAGMA cache_size=-64000;",
    )?;
    Ok(conn)
}

/// Open an in-memory database (for tests).
pub fn open_memory_db() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    Ok(conn)
}

/// Create all tables (without indexes — indexes are created after bulk insert).
pub fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS scan_meta (
            id INTEGER PRIMARY KEY,
            root_path TEXT NOT NULL,
            scanned_at INTEGER NOT NULL,
            total_files INTEGER,
            total_dirs INTEGER,
            total_size INTEGER,
            scan_duration_ms INTEGER
        );

        CREATE TABLE IF NOT EXISTS dirs (
            id INTEGER PRIMARY KEY,
            parent_id INTEGER REFERENCES dirs(id),
            name TEXT NOT NULL,
            file_count INTEGER DEFAULT 0,
            total_file_count INTEGER DEFAULT 0,
            total_logical_size INTEGER DEFAULT 0,
            total_disk_size INTEGER DEFAULT 0,
            created_at INTEGER,
            modified_at INTEGER
        );

        CREATE TABLE IF NOT EXISTS files (
            id INTEGER PRIMARY KEY,
            dir_id INTEGER NOT NULL REFERENCES dirs(id),
            name TEXT NOT NULL,
            logical_size INTEGER NOT NULL,
            disk_size INTEGER NOT NULL,
            created_at INTEGER,
            modified_at INTEGER,
            extension TEXT,
            inode INTEGER,
            content_hash TEXT
        );",
    )?;
    Ok(())
}

/// Create indexes after bulk insert for performance.
pub fn create_indexes(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_files_size ON files(disk_size DESC);
         CREATE INDEX IF NOT EXISTS idx_files_created ON files(created_at);
         CREATE INDEX IF NOT EXISTS idx_files_modified ON files(modified_at);
         CREATE INDEX IF NOT EXISTS idx_files_extension ON files(extension);
         CREATE INDEX IF NOT EXISTS idx_files_dir ON files(dir_id);
         CREATE INDEX IF NOT EXISTS idx_dirs_parent ON dirs(parent_id);
         CREATE INDEX IF NOT EXISTS idx_dirs_size ON dirs(total_disk_size DESC);
         CREATE INDEX IF NOT EXISTS idx_files_hash ON files(content_hash) WHERE content_hash IS NOT NULL;",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_tables_and_indexes() {
        let conn = open_memory_db().unwrap();
        create_tables(&conn).unwrap();
        create_indexes(&conn).unwrap();

        // Verify tables exist
        let tables: Vec<String> = conn
            .prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();

        assert!(tables.contains(&"scan_meta".to_string()));
        assert!(tables.contains(&"dirs".to_string()));
        assert!(tables.contains(&"files".to_string()));
    }

    #[test]
    fn test_insert_and_query_dir() {
        let conn = open_memory_db().unwrap();
        create_tables(&conn).unwrap();

        conn.execute(
            "INSERT INTO dirs (id, parent_id, name, total_disk_size) VALUES (1, NULL, 'root', 1000)",
            [],
        )
        .unwrap();

        let size: i64 = conn
            .query_row("SELECT total_disk_size FROM dirs WHERE id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();

        assert_eq!(size, 1000);
    }
}
```

- [ ] **Step 2: Create cache mod.rs**

Create `src/cache/mod.rs`:

```rust
pub mod schema;
pub mod writer;
pub mod reader;

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Get the cache directory path (~/.diskcopilot/cache/).
pub fn cache_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot find home directory"))?;
    let dir = home.join(".diskcopilot").join("cache");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Get the database path for a given scan root.
/// Uses a hash of the absolute path to derive a unique filename.
pub fn db_path_for(root: &Path) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(root)
        .unwrap_or_else(|_| root.to_path_buf());
    let hash = blake3::hash(canonical.to_string_lossy().as_bytes());
    let filename = format!("{}.db", &hash.to_hex()[..16]);
    Ok(cache_dir()?.join(filename))
}
```

- [ ] **Step 3: Register modules in main.rs**

Add to top of `src/main.rs`:

```rust
mod cache;
mod format;
```

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: All tests pass (format + schema tests).

- [ ] **Step 5: Commit**

```bash
git add src/cache/
git commit -m "feat: SQLite cache schema with tables and deferred indexes"
```

---

## Task 3: Cache Writer (Bulk Inserts)

**Files:**
- Create: `src/cache/writer.rs`

- [ ] **Step 1: Write the cache writer**

Create `src/cache/writer.rs`:

```rust
use anyhow::Result;
use rusqlite::Connection;

/// Entry types for bulk insertion.
pub struct DirEntry {
    pub id: i64,
    pub parent_id: Option<i64>,
    pub name: String,
    pub created_at: Option<i64>,
    pub modified_at: Option<i64>,
}

pub struct FileEntry {
    pub dir_id: i64,
    pub name: String,
    pub logical_size: u64,
    pub disk_size: u64,
    pub created_at: Option<i64>,
    pub modified_at: Option<i64>,
    pub extension: Option<String>,
    pub inode: Option<u64>,
}

pub struct ScanMeta {
    pub root_path: String,
    pub scanned_at: i64,
    pub total_files: u64,
    pub total_dirs: u64,
    pub total_size: u64,
    pub scan_duration_ms: u64,
}

/// Batch writer that buffers entries and flushes in transactions.
pub struct CacheWriter<'a> {
    conn: &'a Connection,
    dir_buffer: Vec<DirEntry>,
    file_buffer: Vec<FileEntry>,
    batch_size: usize,
}

impl<'a> CacheWriter<'a> {
    pub fn new(conn: &'a Connection, batch_size: usize) -> Self {
        Self {
            conn,
            dir_buffer: Vec::with_capacity(batch_size),
            file_buffer: Vec::with_capacity(batch_size),
            batch_size,
        }
    }

    pub fn add_dir(&mut self, entry: DirEntry) -> Result<()> {
        self.dir_buffer.push(entry);
        if self.dir_buffer.len() >= self.batch_size {
            self.flush_dirs()?;
        }
        Ok(())
    }

    pub fn add_file(&mut self, entry: FileEntry) -> Result<()> {
        self.file_buffer.push(entry);
        if self.file_buffer.len() >= self.batch_size {
            self.flush_files()?;
        }
        Ok(())
    }

    pub fn flush_dirs(&mut self) -> Result<()> {
        if self.dir_buffer.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO dirs (id, parent_id, name, created_at, modified_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for dir in self.dir_buffer.drain(..) {
                stmt.execute(rusqlite::params![
                    dir.id,
                    dir.parent_id,
                    dir.name,
                    dir.created_at,
                    dir.modified_at,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn flush_files(&mut self) -> Result<()> {
        if self.file_buffer.is_empty() {
            return Ok(());
        }
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO files (dir_id, name, logical_size, disk_size, created_at, modified_at, extension, inode)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for file in self.file_buffer.drain(..) {
                stmt.execute(rusqlite::params![
                    file.dir_id,
                    file.name,
                    file.logical_size as i64,
                    file.disk_size as i64,
                    file.created_at,
                    file.modified_at,
                    file.extension,
                    file.inode.map(|i| i as i64),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Flush remaining buffers and update directory aggregate sizes.
    pub fn finalize(&mut self) -> Result<()> {
        self.flush_dirs()?;
        self.flush_files()?;
        self.compute_dir_sizes()?;
        Ok(())
    }

    /// Bottom-up aggregation of directory sizes from files and child dirs.
    fn compute_dir_sizes(&self) -> Result<()> {
        // First: set direct file counts and sizes
        self.conn.execute_batch(
            "UPDATE dirs SET
                file_count = (SELECT COUNT(*) FROM files WHERE files.dir_id = dirs.id),
                total_logical_size = COALESCE((SELECT SUM(logical_size) FROM files WHERE files.dir_id = dirs.id), 0),
                total_disk_size = COALESCE((SELECT SUM(disk_size) FROM files WHERE files.dir_id = dirs.id), 0),
                total_file_count = (SELECT COUNT(*) FROM files WHERE files.dir_id = dirs.id);",
        )?;

        // Iterative bottom-up: add child dir sizes to parents.
        // Loop until no more updates (handles arbitrary depth).
        loop {
            let updated = self.conn.execute(
                "UPDATE dirs SET
                    total_logical_size = total_logical_size + COALESCE(
                        (SELECT SUM(d2.total_logical_size) FROM dirs d2 WHERE d2.parent_id = dirs.id), 0),
                    total_disk_size = total_disk_size + COALESCE(
                        (SELECT SUM(d2.total_disk_size) FROM dirs d2 WHERE d2.parent_id = dirs.id), 0),
                    total_file_count = total_file_count + COALESCE(
                        (SELECT SUM(d2.total_file_count) FROM dirs d2 WHERE d2.parent_id = dirs.id), 0)
                 WHERE EXISTS (SELECT 1 FROM dirs d2 WHERE d2.parent_id = dirs.id AND d2.total_disk_size > 0)",
                [],
            )?;
            if updated == 0 {
                break;
            }
        }

        Ok(())
    }

    pub fn write_meta(&self, meta: ScanMeta) -> Result<()> {
        self.conn.execute(
            "INSERT INTO scan_meta (root_path, scanned_at, total_files, total_dirs, total_size, scan_duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                meta.root_path,
                meta.scanned_at,
                meta.total_files as i64,
                meta.total_dirs as i64,
                meta.total_size as i64,
                meta.scan_duration_ms as i64,
            ],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::schema::{create_tables, open_memory_db};

    #[test]
    fn test_bulk_insert_and_finalize() {
        let conn = open_memory_db().unwrap();
        create_tables(&conn).unwrap();

        let mut writer = CacheWriter::new(&conn, 100);

        // Root dir
        writer
            .add_dir(DirEntry {
                id: 1,
                parent_id: None,
                name: "root".into(),
                created_at: Some(1000),
                modified_at: Some(2000),
            })
            .unwrap();

        // Child dir
        writer
            .add_dir(DirEntry {
                id: 2,
                parent_id: Some(1),
                name: "subdir".into(),
                created_at: Some(1000),
                modified_at: Some(2000),
            })
            .unwrap();

        // File in root
        writer
            .add_file(FileEntry {
                dir_id: 1,
                name: "readme.md".into(),
                logical_size: 1024,
                disk_size: 4096,
                created_at: Some(1000),
                modified_at: Some(2000),
                extension: Some("md".into()),
                inode: Some(12345),
            })
            .unwrap();

        // File in subdir
        writer
            .add_file(FileEntry {
                dir_id: 2,
                name: "data.bin".into(),
                logical_size: 1_000_000,
                disk_size: 1_003_520,
                created_at: Some(1000),
                modified_at: Some(2000),
                extension: Some("bin".into()),
                inode: Some(12346),
            })
            .unwrap();

        writer.finalize().unwrap();

        // Root dir should have rolled-up sizes
        let root_size: i64 = conn
            .query_row(
                "SELECT total_disk_size FROM dirs WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();

        // root = own file (4096) + subdir (1_003_520)
        assert_eq!(root_size, 4096 + 1_003_520);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test`
Expected: All tests pass including bulk insert test.

- [ ] **Step 3: Commit**

```bash
git add src/cache/writer.rs
git commit -m "feat: cache writer with batched bulk inserts and dir size aggregation"
```

---

## Task 4: Scanner — Parallel Walk + Metadata

**Files:**
- Create: `src/scanner/mod.rs`
- Create: `src/scanner/walker.rs`
- Create: `src/scanner/metadata.rs`
- Create: `src/scanner/safety.rs`

- [ ] **Step 1: Write metadata extraction**

Create `src/scanner/metadata.rs`:

```rust
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// Extracted file metadata from a single lstat call.
pub struct FileMeta {
    pub logical_size: u64,
    pub disk_size: u64,
    pub created_at: i64,
    pub modified_at: i64,
    pub inode: u64,
    pub is_dir: bool,
    pub is_symlink: bool,
}

/// Extract metadata from a path using lstat (does not follow symlinks).
pub fn extract_metadata(path: &Path) -> std::io::Result<FileMeta> {
    let meta = path.symlink_metadata()?;

    Ok(FileMeta {
        logical_size: meta.len(),
        disk_size: meta.blocks() * 512,
        created_at: meta.ctime(),
        modified_at: meta.mtime(),
        inode: meta.ino(),
        is_dir: meta.is_dir(),
        is_symlink: meta.is_symlink(),
    })
}

/// Extract the file extension from a filename.
pub fn file_extension(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_extract_metadata_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello world").unwrap();

        let meta = extract_metadata(&file_path).unwrap();
        assert_eq!(meta.logical_size, 11);
        assert!(!meta.is_dir);
        assert!(!meta.is_symlink);
        assert!(meta.inode > 0);
        assert!(meta.disk_size > 0);
    }

    #[test]
    fn test_extract_metadata_dir() {
        let dir = tempdir().unwrap();
        let meta = extract_metadata(dir.path()).unwrap();
        assert!(meta.is_dir);
    }

    #[test]
    fn test_file_extension() {
        assert_eq!(file_extension("main.rs"), Some("rs".to_string()));
        assert_eq!(file_extension("archive.tar.gz"), Some("gz".to_string()));
        assert_eq!(file_extension("Makefile"), None);
        assert_eq!(file_extension(".gitignore"), Some("gitignore".to_string()));
    }
}
```

- [ ] **Step 2: Write safety blocklist**

Create `src/scanner/safety.rs`:

```rust
/// Paths that must never be deleted — system-critical directories.
const DANGEROUS_PATHS: &[&str] = &[
    "/",
    "/System",
    "/Library",
    "/usr",
    "/bin",
    "/sbin",
    "/var",
    "/private",
    "/etc",
    "/tmp",
    "/Applications",
    "/Users",
    "/Volumes",
    "/cores",
    "/opt",
];

/// Check if a path is dangerous to delete.
pub fn is_dangerous_path(path: &std::path::Path) -> bool {
    let path_str = path.to_string_lossy();

    // Block paths with fewer than 3 components (e.g., /, /Users)
    if path.components().count() < 3 {
        return true;
    }

    // Block known system directories
    for dangerous in DANGEROUS_PATHS {
        if path_str.as_ref() == *dangerous {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_dangerous_paths() {
        assert!(is_dangerous_path(Path::new("/")));
        assert!(is_dangerous_path(Path::new("/System")));
        assert!(is_dangerous_path(Path::new("/Users")));
        assert!(is_dangerous_path(Path::new("/Applications")));
    }

    #[test]
    fn test_safe_paths() {
        assert!(!is_dangerous_path(Path::new("/Users/dan/Downloads/junk")));
        assert!(!is_dangerous_path(Path::new(
            "/Users/dan/Library/Caches/old"
        )));
    }

    #[test]
    fn test_short_paths_are_dangerous() {
        assert!(is_dangerous_path(Path::new("/foo")));
        assert!(is_dangerous_path(Path::new("/anything")));
    }
}
```

- [ ] **Step 3: Write parallel walker**

Create `src/scanner/walker.rs`:

```rust
use crate::cache::writer::{CacheWriter, DirEntry, FileEntry};
use crate::scanner::metadata::{extract_metadata, file_extension};
use anyhow::Result;
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Progress counters shared between scanner and UI.
pub struct ScanProgress {
    pub files_found: AtomicU64,
    pub dirs_found: AtomicU64,
    pub total_size: AtomicU64,
}

impl ScanProgress {
    pub fn new() -> Self {
        Self {
            files_found: AtomicU64::new(0),
            dirs_found: AtomicU64::new(0),
            total_size: AtomicU64::new(0),
        }
    }
}

/// Configuration for the scanner.
pub struct ScanConfig {
    pub min_file_size: u64,
    pub cache_files: bool, // false = dirs-only mode
    pub full: bool,        // true = cache all files regardless of size
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            min_file_size: 1024 * 1024, // 1MB
            cache_files: true,
            full: false,
        }
    }
}

/// Scan a directory tree in parallel, writing results to SQLite cache.
pub fn scan_directory(
    root: &Path,
    config: &ScanConfig,
    writer: &mut CacheWriter,
    progress: &ScanProgress,
) -> Result<()> {
    let start = Instant::now();
    let seen_inodes: Arc<Mutex<HashSet<u64>>> = Arc::new(Mutex::new(HashSet::new()));

    // We need to assign dir IDs sequentially. Use an atomic counter.
    let next_dir_id = AtomicU64::new(1);

    // Map from path → dir_id for parent lookups.
    let dir_ids: Arc<Mutex<std::collections::HashMap<std::path::PathBuf, i64>>> =
        Arc::new(Mutex::new(std::collections::HashMap::new()));

    // Insert root directory
    let root_canonical = std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let root_id = next_dir_id.fetch_add(1, Ordering::SeqCst) as i64;

    let root_name = root_canonical
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| root_canonical.to_string_lossy().to_string());

    let root_meta = extract_metadata(&root_canonical).ok();

    writer.add_dir(DirEntry {
        id: root_id,
        parent_id: None,
        name: root_name,
        created_at: root_meta.as_ref().map(|m| m.created_at),
        modified_at: root_meta.as_ref().map(|m| m.modified_at),
    })?;

    dir_ids
        .lock()
        .unwrap()
        .insert(root_canonical.clone(), root_id);
    progress.dirs_found.fetch_add(1, Ordering::Relaxed);

    // Parallel walk using jwalk
    for entry in jwalk::WalkDir::new(&root_canonical)
        .skip_hidden(false)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        // Skip the root itself (already inserted)
        if path == root_canonical {
            continue;
        }

        let meta = match extract_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue, // permission denied, etc.
        };

        // Skip symlinks
        if meta.is_symlink {
            continue;
        }

        // Inode dedup for hard links
        {
            let mut seen = seen_inodes.lock().unwrap();
            if !meta.is_dir && !seen.insert(meta.inode) {
                continue; // Already counted this inode
            }
        }

        if meta.is_dir {
            let dir_id = next_dir_id.fetch_add(1, Ordering::SeqCst) as i64;
            let parent_path = path.parent().unwrap_or(&root_canonical);
            let parent_id = dir_ids.lock().unwrap().get(parent_path).copied();

            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            writer.add_dir(DirEntry {
                id: dir_id,
                parent_id,
                name,
                created_at: Some(meta.created_at),
                modified_at: Some(meta.modified_at),
            })?;

            dir_ids.lock().unwrap().insert(path.to_path_buf(), dir_id);
            progress.dirs_found.fetch_add(1, Ordering::Relaxed);
        } else {
            // File — check size threshold
            let should_cache = config.full
                || !config.cache_files
                || meta.disk_size >= config.min_file_size;

            if should_cache && config.cache_files {
                let parent_path = path.parent().unwrap_or(&root_canonical);
                let dir_id = dir_ids
                    .lock()
                    .unwrap()
                    .get(parent_path)
                    .copied()
                    .unwrap_or(root_id);

                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                let ext = file_extension(&name);

                writer.add_file(FileEntry {
                    dir_id,
                    name,
                    logical_size: meta.logical_size,
                    disk_size: meta.disk_size,
                    created_at: Some(meta.created_at),
                    modified_at: Some(meta.modified_at),
                    extension: ext,
                    inode: Some(meta.inode),
                })?;
            }

            progress.files_found.fetch_add(1, Ordering::Relaxed);
            progress
                .total_size
                .fetch_add(meta.disk_size, Ordering::Relaxed);
        }
    }

    Ok(())
}
```

- [ ] **Step 4: Create scanner mod.rs**

Create `src/scanner/mod.rs`:

```rust
pub mod metadata;
pub mod safety;
pub mod walker;
```

- [ ] **Step 5: Register scanner module in main.rs**

Add to top of `src/main.rs`:

```rust
mod cache;
mod format;
mod scanner;
```

- [ ] **Step 6: Run tests**

Run: `cargo test`
Expected: All tests pass (format + schema + writer + metadata + safety tests).

- [ ] **Step 7: Commit**

```bash
git add src/scanner/
git commit -m "feat: parallel filesystem scanner with inode dedup and safety blocklist"
```

---

## Task 5: Wire Scan Command

**Files:**
- Modify: `src/main.rs`
- Create: `tests/scanner_test.rs`

- [ ] **Step 1: Write integration test for scan pipeline**

Create `tests/scanner_test.rs`:

```rust
use std::fs;
use tempfile::tempdir;

#[test]
fn test_scan_creates_cache_and_counts_files() {
    let scan_dir = tempdir().unwrap();
    let cache_dir = tempdir().unwrap();

    // Create test file structure
    fs::write(scan_dir.path().join("file1.txt"), "hello").unwrap();
    fs::write(scan_dir.path().join("file2.rs"), "fn main() {}").unwrap();
    let sub = scan_dir.path().join("subdir");
    fs::create_dir(&sub).unwrap();
    fs::write(sub.join("nested.md"), "# Title").unwrap();

    // Run scan
    let db_path = cache_dir.path().join("test.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    diskcopilot::cache::schema::create_tables(&conn).unwrap();

    let config = diskcopilot::scanner::walker::ScanConfig {
        min_file_size: 0, // cache everything for test
        cache_files: true,
        full: true,
    };

    let progress = diskcopilot::scanner::walker::ScanProgress::new();
    let mut writer = diskcopilot::cache::writer::CacheWriter::new(&conn, 100);

    diskcopilot::scanner::walker::scan_directory(
        scan_dir.path(),
        &config,
        &mut writer,
        &progress,
    )
    .unwrap();

    writer.finalize().unwrap();
    diskcopilot::cache::schema::create_indexes(&conn).unwrap();

    // Verify counts
    let file_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
        .unwrap();
    assert_eq!(file_count, 3); // file1.txt, file2.rs, nested.md

    let dir_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM dirs", [], |row| row.get(0))
        .unwrap();
    assert_eq!(dir_count, 2); // root + subdir
}
```

- [ ] **Step 2: Make crate a library + binary for integration tests**

Add to `Cargo.toml` after `[package]`:

```toml
[[bin]]
name = "diskcopilot"
path = "src/main.rs"

[lib]
name = "diskcopilot"
path = "src/lib.rs"
```

Create `src/lib.rs`:

```rust
pub mod cache;
pub mod format;
pub mod scanner;
```

Update `src/main.rs` to use `diskcopilot::` paths:

```rust
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
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
        } => {
            run_scan(path, full, dirs_only, &min_size).await
        }
        Commands::Tui { path, .. } => {
            println!("TUI: {:?}", path.map(|p| p.display().to_string()));
            Ok(())
        }
    }
}

async fn run_scan(
    path: PathBuf,
    full: bool,
    dirs_only: bool,
    min_size: &str,
) -> anyhow::Result<()> {
    use diskcopilot::cache;
    use diskcopilot::format::{format_size, parse_size};
    use diskcopilot::scanner::walker::{ScanConfig, ScanProgress, scan_directory};

    let min_file_size = if full { 0 } else { parse_size(min_size)? };

    let config = ScanConfig {
        min_file_size,
        cache_files: !dirs_only,
        full,
    };

    let db_path = cache::db_path_for(&path)?;
    // Remove old cache for this path
    if db_path.exists() {
        std::fs::remove_file(&db_path)?;
    }

    let conn = cache::schema::open_db(&db_path)?;
    cache::schema::create_tables(&conn)?;

    let progress = ScanProgress::new();
    let start = Instant::now();

    println!("Scanning: {}", path.display());

    let mut writer = cache::writer::CacheWriter::new(&conn, 5000);
    scan_directory(&path, &config, &mut writer, &progress)?;
    writer.finalize()?;

    let elapsed = start.elapsed();
    cache::schema::create_indexes(&conn)?;

    // Write scan metadata
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;

    writer.write_meta(cache::writer::ScanMeta {
        root_path: path.to_string_lossy().to_string(),
        scanned_at: now,
        total_files: progress.files_found.load(Ordering::Relaxed),
        total_dirs: progress.dirs_found.load(Ordering::Relaxed),
        total_size: progress.total_size.load(Ordering::Relaxed),
        scan_duration_ms: elapsed.as_millis() as u64,
    })?;

    println!(
        "Done: {} files, {} dirs, {} in {:.1}s",
        progress.files_found.load(Ordering::Relaxed),
        progress.dirs_found.load(Ordering::Relaxed),
        format_size(progress.total_size.load(Ordering::Relaxed)),
        elapsed.as_secs_f64(),
    );
    println!("Cache: {}", db_path.display());

    Ok(())
}
```

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All tests pass including the integration test.

Run: `cargo run -- scan /tmp`
Expected: Scans /tmp, prints file/dir/size counts, shows cache path.

- [ ] **Step 4: Commit**

```bash
git add src/main.rs src/lib.rs tests/scanner_test.rs Cargo.toml
git commit -m "feat: wire scan command with progress output and integration test"
```

---

## Task 6: Cache Reader (Tree Reconstruction + View Queries)

**Files:**
- Create: `src/cache/reader.rs`

- [ ] **Step 1: Write cache reader**

Create `src/cache/reader.rs`:

```rust
use anyhow::Result;
use rusqlite::Connection;

/// A node in the reconstructed tree (used by TUI).
#[derive(Debug, Clone)]
pub struct TreeNode {
    pub id: i64,
    pub name: String,
    pub is_dir: bool,
    pub disk_size: u64,
    pub logical_size: u64,
    pub file_count: u64,
    pub created_at: Option<i64>,
    pub modified_at: Option<i64>,
    pub extension: Option<String>,
    pub children: Vec<TreeNode>,
}

/// A flat file entry for list views (Large Files, Recent, Old, etc.).
#[derive(Debug, Clone)]
pub struct FileRow {
    pub name: String,
    pub full_path: String,
    pub disk_size: u64,
    pub logical_size: u64,
    pub created_at: Option<i64>,
    pub modified_at: Option<i64>,
    pub extension: Option<String>,
}

/// Load the children of a directory from cache (lazy loading for TUI).
pub fn load_children(conn: &Connection, dir_id: i64) -> Result<Vec<TreeNode>> {
    let mut children = Vec::new();

    // Child directories
    let mut dir_stmt = conn.prepare_cached(
        "SELECT id, name, total_disk_size, total_logical_size, total_file_count, created_at, modified_at
         FROM dirs WHERE parent_id = ?1 ORDER BY total_disk_size DESC",
    )?;

    let dir_rows = dir_stmt.query_map([dir_id], |row| {
        Ok(TreeNode {
            id: row.get(0)?,
            name: row.get(1)?,
            is_dir: true,
            disk_size: row.get::<_, i64>(2)? as u64,
            logical_size: row.get::<_, i64>(3)? as u64,
            file_count: row.get::<_, i64>(4)? as u64,
            created_at: row.get(5)?,
            modified_at: row.get(6)?,
            extension: None,
            children: Vec::new(), // loaded lazily
        })
    })?;

    for row in dir_rows {
        children.push(row?);
    }

    // Child files
    let mut file_stmt = conn.prepare_cached(
        "SELECT id, name, disk_size, logical_size, created_at, modified_at, extension
         FROM files WHERE dir_id = ?1 ORDER BY disk_size DESC",
    )?;

    let file_rows = file_stmt.query_map([dir_id], |row| {
        Ok(TreeNode {
            id: row.get(0)?,
            name: row.get(1)?,
            is_dir: false,
            disk_size: row.get::<_, i64>(2)? as u64,
            logical_size: row.get::<_, i64>(3)? as u64,
            file_count: 0,
            created_at: row.get(4)?,
            modified_at: row.get(5)?,
            extension: row.get(6)?,
            children: Vec::new(),
        })
    })?;

    for row in file_rows {
        children.push(row?);
    }

    Ok(children)
}

/// Get the root directory node.
pub fn load_root(conn: &Connection) -> Result<TreeNode> {
    let node = conn.query_row(
        "SELECT id, name, total_disk_size, total_logical_size, total_file_count, created_at, modified_at
         FROM dirs WHERE parent_id IS NULL LIMIT 1",
        [],
        |row| {
            Ok(TreeNode {
                id: row.get(0)?,
                name: row.get(1)?,
                is_dir: true,
                disk_size: row.get::<_, i64>(2)? as u64,
                logical_size: row.get::<_, i64>(3)? as u64,
                file_count: row.get::<_, i64>(4)? as u64,
                created_at: row.get(5)?,
                modified_at: row.get(6)?,
                extension: None,
                children: Vec::new(),
            })
        },
    )?;

    Ok(node)
}

/// Reconstruct the full path for a directory by walking parent_id up the tree.
pub fn reconstruct_path(conn: &Connection, dir_id: i64) -> Result<String> {
    let mut parts = Vec::new();
    let mut current_id = Some(dir_id);

    while let Some(id) = current_id {
        let (name, parent): (String, Option<i64>) = conn.query_row(
            "SELECT name, parent_id FROM dirs WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        parts.push(name);
        current_id = parent;
    }

    parts.reverse();
    Ok(parts.join("/"))
}

// --- View Queries ---

/// Large files view: files above a size threshold.
pub fn query_large_files(conn: &Connection, min_size: u64, limit: usize) -> Result<Vec<FileRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT f.name, f.disk_size, f.logical_size, f.created_at, f.modified_at, f.extension, f.dir_id
         FROM files f
         WHERE f.disk_size >= ?1
         ORDER BY f.disk_size DESC
         LIMIT ?2",
    )?;

    let rows = stmt.query_map(rusqlite::params![min_size as i64, limit as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, i64>(6)?,
        ))
    })?;

    let mut result = Vec::new();
    for row in rows {
        let (name, disk_size, logical_size, created_at, modified_at, extension, dir_id) = row?;
        let dir_path = reconstruct_path(conn, dir_id)?;
        result.push(FileRow {
            full_path: format!("{}/{}", dir_path, name),
            name,
            disk_size: disk_size as u64,
            logical_size: logical_size as u64,
            created_at,
            modified_at,
            extension,
        });
    }

    Ok(result)
}

/// Recent files view: files modified after a given timestamp.
pub fn query_recent_files(
    conn: &Connection,
    after_timestamp: i64,
    limit: usize,
) -> Result<Vec<FileRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT f.name, f.disk_size, f.logical_size, f.created_at, f.modified_at, f.extension, f.dir_id
         FROM files f
         WHERE f.modified_at >= ?1
         ORDER BY f.modified_at DESC
         LIMIT ?2",
    )?;

    let rows = stmt.query_map(rusqlite::params![after_timestamp, limit as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, i64>(6)?,
        ))
    })?;

    let mut result = Vec::new();
    for row in rows {
        let (name, disk_size, logical_size, created_at, modified_at, extension, dir_id) = row?;
        let dir_path = reconstruct_path(conn, dir_id)?;
        result.push(FileRow {
            full_path: format!("{}/{}", dir_path, name),
            name,
            disk_size: disk_size as u64,
            logical_size: logical_size as u64,
            created_at,
            modified_at,
            extension,
        });
    }

    Ok(result)
}

/// Old files view: files not modified since before a given timestamp, largest first.
pub fn query_old_files(
    conn: &Connection,
    before_timestamp: i64,
    limit: usize,
) -> Result<Vec<FileRow>> {
    let mut stmt = conn.prepare_cached(
        "SELECT f.name, f.disk_size, f.logical_size, f.created_at, f.modified_at, f.extension, f.dir_id
         FROM files f
         WHERE f.modified_at < ?1
         ORDER BY f.disk_size DESC
         LIMIT ?2",
    )?;

    let rows = stmt.query_map(rusqlite::params![before_timestamp, limit as i64], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, i64>(2)?,
            row.get::<_, Option<i64>>(3)?,
            row.get::<_, Option<i64>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, i64>(6)?,
        ))
    })?;

    let mut result = Vec::new();
    for row in rows {
        let (name, disk_size, logical_size, created_at, modified_at, extension, dir_id) = row?;
        let dir_path = reconstruct_path(conn, dir_id)?;
        result.push(FileRow {
            full_path: format!("{}/{}", dir_path, name),
            name,
            disk_size: disk_size as u64,
            logical_size: logical_size as u64,
            created_at,
            modified_at,
            extension,
        });
    }

    Ok(result)
}

/// Dev artifacts view: directories matching known dev artifact names.
pub fn query_dev_artifacts(conn: &Connection) -> Result<Vec<TreeNode>> {
    let artifacts = [
        "node_modules",
        "target",
        ".next",
        "__pycache__",
        ".build",
        "Pods",
        ".gradle",
        "build",
        "dist",
        ".cache",
        ".parcel-cache",
        ".turbo",
        "vendor",
    ];

    let placeholders: Vec<String> = (0..artifacts.len()).map(|i| format!("?{}", i + 1)).collect();
    let sql = format!(
        "SELECT id, name, total_disk_size, total_logical_size, total_file_count, created_at, modified_at
         FROM dirs WHERE name IN ({}) ORDER BY total_disk_size DESC",
        placeholders.join(", ")
    );

    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::types::ToSql> =
        artifacts.iter().map(|a| a as &dyn rusqlite::types::ToSql).collect();

    let rows = stmt.query_map(params.as_slice(), |row| {
        Ok(TreeNode {
            id: row.get(0)?,
            name: row.get(1)?,
            is_dir: true,
            disk_size: row.get::<_, i64>(2)? as u64,
            logical_size: row.get::<_, i64>(3)? as u64,
            file_count: row.get::<_, i64>(4)? as u64,
            created_at: row.get(5)?,
            modified_at: row.get(6)?,
            extension: None,
            children: Vec::new(),
        })
    })?;

    rows.collect::<std::result::Result<Vec<_>, _>>().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::schema::{create_indexes, create_tables, open_memory_db};
    use crate::cache::writer::{CacheWriter, DirEntry, FileEntry};

    fn setup_test_db() -> Connection {
        let conn = open_memory_db().unwrap();
        create_tables(&conn).unwrap();

        let mut writer = CacheWriter::new(&conn, 100);

        writer
            .add_dir(DirEntry {
                id: 1,
                parent_id: None,
                name: "home".into(),
                created_at: Some(1000),
                modified_at: Some(2000),
            })
            .unwrap();

        writer
            .add_dir(DirEntry {
                id: 2,
                parent_id: Some(1),
                name: "projects".into(),
                created_at: Some(1000),
                modified_at: Some(2000),
            })
            .unwrap();

        writer
            .add_dir(DirEntry {
                id: 3,
                parent_id: Some(2),
                name: "node_modules".into(),
                created_at: Some(1000),
                modified_at: Some(2000),
            })
            .unwrap();

        writer
            .add_file(FileEntry {
                dir_id: 1,
                name: "big.zip".into(),
                logical_size: 500_000_000,
                disk_size: 500_000_000,
                created_at: Some(1000),
                modified_at: Some(9000), // recent
                extension: Some("zip".into()),
                inode: Some(1),
            })
            .unwrap();

        writer
            .add_file(FileEntry {
                dir_id: 2,
                name: "old.log".into(),
                logical_size: 100_000,
                disk_size: 102_400,
                created_at: Some(100),
                modified_at: Some(100), // very old
                extension: Some("log".into()),
                inode: Some(2),
            })
            .unwrap();

        writer
            .add_file(FileEntry {
                dir_id: 3,
                name: "react.js".into(),
                logical_size: 2_000_000,
                disk_size: 2_000_000,
                created_at: Some(1000),
                modified_at: Some(1000),
                extension: Some("js".into()),
                inode: Some(3),
            })
            .unwrap();

        writer.finalize().unwrap();
        create_indexes(&conn).unwrap();
        conn
    }

    #[test]
    fn test_load_root() {
        let conn = setup_test_db();
        let root = load_root(&conn).unwrap();
        assert_eq!(root.name, "home");
        assert!(root.disk_size > 0);
    }

    #[test]
    fn test_load_children() {
        let conn = setup_test_db();
        let children = load_children(&conn, 1).unwrap();
        // Should have: projects dir + big.zip file
        assert_eq!(children.len(), 2);
        assert!(children.iter().any(|c| c.name == "projects" && c.is_dir));
        assert!(children.iter().any(|c| c.name == "big.zip" && !c.is_dir));
    }

    #[test]
    fn test_query_large_files() {
        let conn = setup_test_db();
        let large = query_large_files(&conn, 1_000_000, 100).unwrap();
        assert_eq!(large.len(), 2); // big.zip (500MB) + react.js (2MB)
        assert_eq!(large[0].name, "big.zip"); // sorted by size desc
    }

    #[test]
    fn test_query_recent_files() {
        let conn = setup_test_db();
        let recent = query_recent_files(&conn, 5000, 100).unwrap();
        assert_eq!(recent.len(), 1); // only big.zip was modified at 9000
        assert_eq!(recent[0].name, "big.zip");
    }

    #[test]
    fn test_query_old_files() {
        let conn = setup_test_db();
        let old = query_old_files(&conn, 500, 100).unwrap();
        assert_eq!(old.len(), 1); // only old.log was modified at 100
        assert_eq!(old[0].name, "old.log");
    }

    #[test]
    fn test_query_dev_artifacts() {
        let conn = setup_test_db();
        let artifacts = query_dev_artifacts(&conn).unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].name, "node_modules");
    }

    #[test]
    fn test_reconstruct_path() {
        let conn = setup_test_db();
        let path = reconstruct_path(&conn, 3).unwrap();
        assert_eq!(path, "home/projects/node_modules");
    }
}
```

- [ ] **Step 2: Update lib.rs**

Add `pub mod cache;` already exists, but make sure `reader` is accessible. The `src/cache/mod.rs` already has `pub mod reader;`.

- [ ] **Step 3: Run tests**

Run: `cargo test`
Expected: All tests pass including reader view query tests.

- [ ] **Step 4: Commit**

```bash
git add src/cache/reader.rs
git commit -m "feat: cache reader with tree reconstruction and view queries"
```

---

## Task 7: Config Module

**Files:**
- Create: `src/config/mod.rs`
- Create: `src/config/loader.rs`

- [ ] **Step 1: Write config loader**

Create `src/config/loader.rs`:

```rust
use anyhow::Result;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub scan: ScanConfig,
    #[serde(default)]
    pub tui: TuiConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ScanConfig {
    #[serde(default = "default_min_size")]
    pub default_min_size: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TuiConfig {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_large_file_threshold")]
    pub large_file_threshold: String,
    #[serde(default = "default_recent_days")]
    pub recent_days: u32,
    #[serde(default = "default_old_days")]
    pub old_days: u32,
}

fn default_min_size() -> String { "1M".into() }
fn default_theme() -> String { "dark".into() }
fn default_large_file_threshold() -> String { "500M".into() }
fn default_recent_days() -> u32 { 7 }
fn default_old_days() -> u32 { 365 }

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            default_min_size: default_min_size(),
        }
    }
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            large_file_threshold: default_large_file_threshold(),
            recent_days: default_recent_days(),
            old_days: default_old_days(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scan: ScanConfig::default(),
            tui: TuiConfig::default(),
        }
    }
}

/// Get config file path (~/.diskcopilot/config.toml).
pub fn config_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot find home directory"))?;
    Ok(home.join(".diskcopilot").join("config.toml"))
}

/// Load config from file, falling back to defaults.
pub fn load_config() -> Result<Config> {
    let path = config_path()?;
    if path.exists() {
        let content = std::fs::read_to_string(&path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    } else {
        Ok(Config::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.tui.theme, "dark");
        assert_eq!(config.tui.recent_days, 7);
        assert_eq!(config.tui.old_days, 365);
    }

    #[test]
    fn test_parse_config_toml() {
        let toml_str = r#"
            [tui]
            theme = "dracula"
            large_file_threshold = "1G"
            recent_days = 14
        "#;
        let config: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(config.tui.theme, "dracula");
        assert_eq!(config.tui.large_file_threshold, "1G");
        assert_eq!(config.tui.recent_days, 14);
        assert_eq!(config.tui.old_days, 365); // default
    }
}
```

- [ ] **Step 2: Create config mod.rs**

Create `src/config/mod.rs`:

```rust
pub mod loader;
```

- [ ] **Step 3: Add to lib.rs**

Add `pub mod config;` to `src/lib.rs`.

- [ ] **Step 4: Run tests**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add src/config/
git commit -m "feat: config module with TOML loading and sensible defaults"
```

---

## Task 8: TUI Foundation — Event System + App Shell

**Files:**
- Create: `src/tui/mod.rs`
- Create: `src/tui/event.rs`
- Create: `src/tui/app.rs`
- Create: `src/tui/theme.rs`
- Create: `src/tui/icons.rs`

- [ ] **Step 1: Write the event system**

Create `src/tui/event.rs`:

```rust
use crossterm::event::KeyEvent;
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::sync::mpsc;

/// Atomic render flag: 0 = none, 1 = full, 2 = partial
pub static NEED_RENDER: AtomicU8 = AtomicU8::new(0);

pub fn request_render(partial: bool) {
    let val = if partial { 2 } else { 1 };
    NEED_RENDER.fetch_max(val, Ordering::Relaxed);
}

#[derive(Debug)]
pub enum Event {
    Key(KeyEvent),
    Resize(u16, u16),
    ScanProgress {
        files: u64,
        dirs: u64,
        total_size: u64,
    },
    ScanComplete,
    Tick,
}

pub type EventSender = mpsc::UnboundedSender<Event>;
pub type EventReceiver = mpsc::UnboundedReceiver<Event>;

pub fn channel() -> (EventSender, EventReceiver) {
    mpsc::unbounded_channel()
}
```

- [ ] **Step 2: Write theme definitions**

Create `src/tui/theme.rs`:

```rust
use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: String,
    pub bg: Color,
    pub fg: Color,
    pub dir: Style,
    pub file: Style,
    pub file_large: Style,
    pub file_medium: Style,
    pub file_small: Style,
    pub selected: Style,
    pub bar_low: Color,
    pub bar_mid: Color,
    pub bar_high: Color,
    pub tab_active: Style,
    pub tab_inactive: Style,
    pub status_bar: Style,
    pub header: Style,
    pub border: Style,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            name: "dark".into(),
            bg: Color::Reset,
            fg: Color::White,
            dir: Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
            file: Style::default().fg(Color::White),
            file_large: Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            file_medium: Style::default().fg(Color::Yellow),
            file_small: Style::default().fg(Color::DarkGray),
            selected: Style::default().bg(Color::DarkGray).fg(Color::White).add_modifier(Modifier::BOLD),
            bar_low: Color::Green,
            bar_mid: Color::Yellow,
            bar_high: Color::Red,
            tab_active: Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            tab_inactive: Style::default().fg(Color::DarkGray),
            status_bar: Style::default().fg(Color::DarkGray),
            header: Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            border: Style::default().fg(Color::DarkGray),
        }
    }

    pub fn light() -> Self {
        Self {
            name: "light".into(),
            bg: Color::White,
            fg: Color::Black,
            dir: Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
            file: Style::default().fg(Color::Black),
            file_large: Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            file_medium: Style::default().fg(Color::Rgb(180, 120, 0)),
            file_small: Style::default().fg(Color::Gray),
            selected: Style::default().bg(Color::LightBlue).fg(Color::Black).add_modifier(Modifier::BOLD),
            bar_low: Color::Green,
            bar_mid: Color::Rgb(180, 120, 0),
            bar_high: Color::Red,
            tab_active: Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
            tab_inactive: Style::default().fg(Color::Gray),
            status_bar: Style::default().fg(Color::Gray),
            header: Style::default().fg(Color::Blue).add_modifier(Modifier::BOLD),
            border: Style::default().fg(Color::Gray),
        }
    }

    pub fn by_name(name: &str) -> Self {
        match name {
            "light" => Self::light(),
            _ => Self::dark(),
        }
    }

    /// Get the style for a file based on its size relative to the parent total.
    pub fn file_style(&self, size: u64, parent_total: u64) -> Style {
        if parent_total == 0 {
            return self.file;
        }
        let ratio = size as f64 / parent_total as f64;
        if ratio > 0.1 {
            self.file_large
        } else if ratio > 0.01 {
            self.file_medium
        } else {
            self.file_small
        }
    }

    /// Get the bar color based on proportion.
    pub fn bar_color(&self, ratio: f64) -> Color {
        if ratio > 0.5 {
            self.bar_high
        } else if ratio > 0.2 {
            self.bar_mid
        } else {
            self.bar_low
        }
    }
}
```

- [ ] **Step 3: Write Nerd Font icon mapping**

Create `src/tui/icons.rs`:

```rust
/// Get the Nerd Font icon for a file extension or directory.
pub fn icon_for(name: &str, is_dir: bool, is_open: bool) -> &'static str {
    if is_dir {
        return if is_open { "󰝰 " } else { "󰉋 " };
    }

    let ext = name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        // Languages
        "rs" => " ",
        "py" => " ",
        "js" => " ",
        "ts" | "tsx" => " ",
        "jsx" => " ",
        "go" => " ",
        "java" => " ",
        "c" | "h" => " ",
        "cpp" | "hpp" | "cc" => " ",
        "cs" => "󰌛 ",
        "rb" => " ",
        "php" => " ",
        "swift" => " ",
        "kt" => " ",
        "lua" => " ",
        "sh" | "bash" | "zsh" | "fish" => " ",

        // Web
        "html" | "htm" => " ",
        "css" | "scss" | "sass" | "less" => " ",
        "vue" => " ",
        "svelte" => " ",

        // Data
        "json" => " ",
        "yaml" | "yml" => " ",
        "toml" => " ",
        "xml" => "󰗀 ",
        "csv" => " ",
        "sql" | "db" | "sqlite" => " ",

        // Documents
        "md" | "mdx" => " ",
        "txt" => " ",
        "pdf" => " ",
        "doc" | "docx" => "󰈬 ",

        // Images
        "png" | "jpg" | "jpeg" | "gif" | "bmp" | "svg" | "webp" | "ico" => " ",

        // Video
        "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" => " ",

        // Audio
        "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" => " ",

        // Archives
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" | "dmg" => " ",

        // Config
        "env" | "ini" | "cfg" => " ",
        "lock" => " ",

        // Git
        "gitignore" | "gitmodules" | "gitattributes" => " ",

        // macOS
        "app" | "framework" | "bundle" | "kext" => " ",
        "plist" => " ",

        // Binary
        "exe" | "dll" | "so" | "dylib" => " ",
        "o" | "a" => " ",

        // Misc
        "log" => "󰌱 ",
        "tmp" | "bak" | "swp" => " ",
        "dockerfile" => "󰡨 ",

        _ => " ",
    }
}

/// Icon for well-known filenames (not extension-based).
pub fn icon_for_name(name: &str) -> Option<&'static str> {
    match name.to_lowercase().as_str() {
        "makefile" | "cmakelists.txt" => Some(" "),
        "dockerfile" => Some("󰡨 "),
        "license" | "licence" => Some(" "),
        "readme" | "readme.md" => Some("󰂺 "),
        ".gitignore" => Some(" "),
        ".env" => Some(" "),
        "cargo.toml" => Some(" "),
        "package.json" => Some(" "),
        _ => None,
    }
}
```

- [ ] **Step 4: Write the App struct and main event loop**

Create `src/tui/app.rs`:

```rust
use crate::cache::reader::{self, FileRow, TreeNode};
use crate::tui::event::{self, Event, EventReceiver, EventSender, NEED_RENDER};
use crate::tui::theme::Theme;
use anyhow::Result;
use crossterm::{
    event::{self as ct_event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use rusqlite::Connection;
use std::io;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum View {
    Tree,
    LargeFiles,
    Recent,
    Old,
    DevArtifacts,
    Duplicates,
}

impl View {
    pub fn all() -> &'static [View] {
        &[
            View::Tree,
            View::LargeFiles,
            View::Recent,
            View::Old,
            View::DevArtifacts,
            View::Duplicates,
        ]
    }

    pub fn label(&self) -> &'static str {
        match self {
            View::Tree => "Tree",
            View::LargeFiles => "Large Files",
            View::Recent => "Recent",
            View::Old => "Old",
            View::DevArtifacts => "Dev Artifacts",
            View::Duplicates => "Duplicates",
        }
    }

    pub fn next(&self) -> View {
        let views = Self::all();
        let idx = views.iter().position(|v| v == self).unwrap_or(0);
        views[(idx + 1) % views.len()]
    }

    pub fn prev(&self) -> View {
        let views = Self::all();
        let idx = views.iter().position(|v| v == self).unwrap_or(0);
        views[(idx + views.len() - 1) % views.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortMode {
    SizeDesc,
    SizeAsc,
    Name,
    DateModified,
    DateCreated,
}

pub struct App {
    pub should_quit: bool,
    pub view: View,
    pub sort_mode: SortMode,
    pub theme: Theme,
    pub root: Option<TreeNode>,
    pub cursor: usize,
    pub scroll_offset: usize,
    pub expanded: std::collections::HashSet<i64>,
    pub show_detail: bool,
    pub show_help: bool,
    pub visible_items: Vec<VisibleItem>,
    pub list_items: Vec<FileRow>,
    pub confirm_delete: Option<DeleteConfirm>,
    pub marked: std::collections::HashSet<usize>,
    pub root_path: String,
    pub scan_meta: Option<ScanMetaInfo>,
}

#[derive(Debug, Clone)]
pub struct VisibleItem {
    pub node: TreeNode,
    pub depth: usize,
    pub is_expanded: bool,
    pub parent_size: u64,
}

#[derive(Debug)]
pub struct DeleteConfirm {
    pub name: String,
    pub path: String,
    pub size: u64,
    pub is_dir: bool,
}

#[derive(Debug, Clone)]
pub struct ScanMetaInfo {
    pub total_files: u64,
    pub total_dirs: u64,
    pub total_size: u64,
}

impl App {
    pub fn new(theme_name: &str) -> Self {
        Self {
            should_quit: false,
            view: View::Tree,
            sort_mode: SortMode::SizeDesc,
            theme: Theme::by_name(theme_name),
            root: None,
            cursor: 0,
            scroll_offset: 0,
            expanded: std::collections::HashSet::new(),
            show_detail: true,
            show_help: false,
            visible_items: Vec::new(),
            list_items: Vec::new(),
            confirm_delete: None,
            marked: std::collections::HashSet::new(),
            root_path: String::new(),
            scan_meta: None,
        }
    }

    /// Load tree data from the SQLite cache.
    pub fn load_from_cache(&mut self, conn: &Connection) -> Result<()> {
        let root = reader::load_root(conn)?;
        self.root_path = reader::reconstruct_path(conn, root.id)?;

        // Auto-expand root
        self.expanded.insert(root.id);
        self.root = Some(root);

        // Load scan meta
        let meta: (i64, i64, i64) = conn.query_row(
            "SELECT total_files, total_dirs, total_size FROM scan_meta ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;

        self.scan_meta = Some(ScanMetaInfo {
            total_files: meta.0 as u64,
            total_dirs: meta.1 as u64,
            total_size: meta.2 as u64,
        });

        Ok(())
    }

    /// Build the flat list of visible items from the tree for rendering.
    pub fn rebuild_visible(&mut self, conn: &Connection) {
        self.visible_items.clear();
        if let Some(root) = &self.root {
            self.flatten_tree(conn, root.id, 0, root.disk_size);
        }
    }

    fn flatten_tree(&mut self, conn: &Connection, dir_id: i64, depth: usize, parent_size: u64) {
        if let Ok(children) = reader::load_children(conn, dir_id) {
            for child in children {
                let is_expanded = child.is_dir && self.expanded.contains(&child.id);
                self.visible_items.push(VisibleItem {
                    node: child.clone(),
                    depth,
                    is_expanded,
                    parent_size,
                });
                if is_expanded {
                    self.flatten_tree(conn, child.id, depth + 1, child.disk_size);
                }
            }
        }
    }

    pub fn move_cursor(&mut self, delta: i32) {
        let len = self.visible_items.len().max(1);
        let new = (self.cursor as i32 + delta).clamp(0, len as i32 - 1) as usize;
        self.cursor = new;
    }

    pub fn toggle_expand(&mut self, conn: &Connection) {
        if let Some(item) = self.visible_items.get(self.cursor) {
            if item.node.is_dir {
                let id = item.node.id;
                if self.expanded.contains(&id) {
                    self.expanded.remove(&id);
                } else {
                    self.expanded.insert(id);
                }
                self.rebuild_visible(conn);
            }
        }
    }

    pub fn collapse_or_parent(&mut self, conn: &Connection) {
        if let Some(item) = self.visible_items.get(self.cursor) {
            if item.node.is_dir && self.expanded.contains(&item.node.id) {
                // Collapse current
                self.expanded.remove(&item.node.id);
                self.rebuild_visible(conn);
            }
            // TODO: move to parent dir
        }
    }
}

/// Run the TUI application.
pub async fn run(conn: Connection, theme_name: &str) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(theme_name);
    app.load_from_cache(&conn)?;
    app.rebuild_visible(&conn);

    let (tx, mut rx) = event::channel();

    // Spawn crossterm event reader
    let event_tx = tx.clone();
    tokio::spawn(async move {
        loop {
            if ct_event::poll(Duration::from_millis(50)).unwrap_or(false) {
                if let Ok(evt) = ct_event::read() {
                    match evt {
                        ct_event::Event::Key(key) => {
                            let _ = event_tx.send(Event::Key(key));
                        }
                        ct_event::Event::Resize(w, h) => {
                            let _ = event_tx.send(Event::Resize(w, h));
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    // Main loop
    let mut last_render = Instant::now();
    loop {
        // Render if needed (10ms debounce)
        if NEED_RENDER.load(Ordering::Relaxed) > 0 || last_render.elapsed() > Duration::from_millis(100) {
            terminal.draw(|frame| {
                crate::tui::render(frame, &mut app);
            })?;
            NEED_RENDER.store(0, Ordering::Relaxed);
            last_render = Instant::now();
        }

        // Process events
        tokio::select! {
            Some(event) = rx.recv() => {
                match event {
                    Event::Key(key) => {
                        handle_key(&mut app, key, &conn);
                        event::request_render(false);
                    }
                    Event::Resize(_, _) => {
                        event::request_render(false);
                    }
                    _ => {}
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(50)) => {}
        }

        if app.should_quit {
            break;
        }
    }

    // Cleanup
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

fn handle_key(app: &mut App, key: crossterm::event::KeyEvent, conn: &Connection) {
    // Handle confirmation dialog first
    if app.confirm_delete.is_some() {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('d') => {
                // TODO: execute delete
                app.confirm_delete = None;
            }
            KeyCode::Esc | KeyCode::Char('n') => {
                app.confirm_delete = None;
            }
            _ => {}
        }
        return;
    }

    // Handle help overlay
    if app.show_help {
        app.show_help = false;
        return;
    }

    match key.code {
        // Navigation
        KeyCode::Char('j') | KeyCode::Down => app.move_cursor(1),
        KeyCode::Char('k') | KeyCode::Up => app.move_cursor(-1),
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => app.toggle_expand(conn),
        KeyCode::Char('h') | KeyCode::Left => app.collapse_or_parent(conn),
        KeyCode::Char('g') | KeyCode::Home => app.cursor = 0,
        KeyCode::Char('G') | KeyCode::End => {
            app.cursor = app.visible_items.len().saturating_sub(1);
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.move_cursor(20); // page down
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.move_cursor(-20); // page up
        }

        // Views
        KeyCode::Tab => {
            app.view = app.view.next();
            event::request_render(false);
        }
        KeyCode::BackTab => {
            app.view = app.view.prev();
            event::request_render(false);
        }

        // Actions
        KeyCode::Char(' ') => {
            if app.marked.contains(&app.cursor) {
                app.marked.remove(&app.cursor);
            } else {
                app.marked.insert(app.cursor);
            }
            app.move_cursor(1);
        }
        KeyCode::Char('d') => {
            // Delete - show confirmation
            if let Some(item) = app.visible_items.get(app.cursor) {
                app.confirm_delete = Some(DeleteConfirm {
                    name: item.node.name.clone(),
                    path: String::new(), // TODO: reconstruct
                    size: item.node.disk_size,
                    is_dir: item.node.is_dir,
                });
            }
        }

        // View controls
        KeyCode::Char('s') => {
            app.sort_mode = match app.sort_mode {
                SortMode::SizeDesc => SortMode::Name,
                SortMode::Name => SortMode::DateModified,
                SortMode::DateModified => SortMode::DateCreated,
                SortMode::DateCreated => SortMode::SizeAsc,
                SortMode::SizeAsc => SortMode::SizeDesc,
            };
        }
        KeyCode::Char('i') => app.show_detail = !app.show_detail,
        KeyCode::Char('?') => app.show_help = true,
        KeyCode::Char('q') => app.should_quit = true,

        _ => {}
    }
}
```

- [ ] **Step 5: Create tui mod.rs with render function stub**

Create `src/tui/mod.rs`:

```rust
pub mod app;
pub mod event;
pub mod icons;
pub mod theme;

use app::App;
use ratatui::Frame;

/// Top-level render function called by the event loop.
pub fn render(frame: &mut Frame, app: &mut App) {
    // Placeholder — will be implemented in Task 9
    use ratatui::widgets::Paragraph;
    let text = format!(
        "DiskCopilot - {} | View: {} | Items: {} | Press q to quit",
        app.root_path,
        app.view.label(),
        app.visible_items.len(),
    );
    frame.render_widget(Paragraph::new(text), frame.area());
}
```

- [ ] **Step 6: Add tui module to lib.rs**

Update `src/lib.rs`:

```rust
pub mod cache;
pub mod config;
pub mod format;
pub mod scanner;
pub mod tui;
```

- [ ] **Step 7: Wire TUI command in main.rs**

Update the `Commands::Tui` match arm in `src/main.rs`:

```rust
Commands::Tui {
    path,
    cached,
    theme,
    ..
} => {
    use diskcopilot::cache;

    let scan_path = path.unwrap_or_else(|| std::env::current_dir().unwrap());
    let db_path = cache::db_path_for(&scan_path)?;

    if !db_path.exists() && !cached {
        // Scan first
        run_scan(scan_path.clone(), false, false, "1M").await?;
    }

    if !db_path.exists() {
        anyhow::bail!("No cache found for {}. Run 'diskcopilot scan' first.", scan_path.display());
    }

    let conn = cache::schema::open_db(&db_path)?;
    diskcopilot::tui::app::run(conn, &theme).await
}
```

- [ ] **Step 8: Run and verify**

Run: `cargo build`
Expected: Compiles successfully.

Run: `cargo run -- scan /tmp && cargo run -- tui /tmp`
Expected: Scan completes, TUI launches with placeholder text, `q` exits.

- [ ] **Step 9: Commit**

```bash
git add src/tui/ src/lib.rs src/main.rs
git commit -m "feat: TUI foundation with event system, themes, icons, and app shell"
```

---

## Task 9: TUI Tree View Rendering

**Files:**
- Create: `src/tui/tree.rs`
- Create: `src/tui/tabs.rs`
- Create: `src/tui/statusbar.rs`
- Create: `src/tui/detail.rs`
- Modify: `src/tui/mod.rs`

- [ ] **Step 1: Write the tab bar widget**

Create `src/tui/tabs.rs`:

```rust
use crate::tui::app::View;
use crate::tui::theme::Theme;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
};

pub struct TabBar<'a> {
    active: View,
    theme: &'a Theme,
}

impl<'a> TabBar<'a> {
    pub fn new(active: View, theme: &'a Theme) -> Self {
        Self { active, theme }
    }
}

impl Widget for TabBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let mut spans = Vec::new();
        for (i, view) in View::all().iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw(" │ "));
            }
            let style = if *view == self.active {
                self.theme.tab_active
            } else {
                self.theme.tab_inactive
            };
            spans.push(Span::styled(view.label(), style));
        }
        let line = Line::from(spans);
        buf.set_line(area.x, area.y, &line, area.width);
    }
}
```

- [ ] **Step 2: Write the status bar widget**

Create `src/tui/statusbar.rs`:

```rust
use crate::format::format_size;
use crate::tui::app::{App, SortMode};
use crate::tui::theme::Theme;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
};

pub struct StatusBar<'a> {
    app: &'a App,
}

impl<'a> StatusBar<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let meta = self.app.scan_meta.as_ref();
        let sort_label = match self.app.sort_mode {
            SortMode::SizeDesc => "size ↓",
            SortMode::SizeAsc => "size ↑",
            SortMode::Name => "name",
            SortMode::DateModified => "modified",
            SortMode::DateCreated => "created",
        };

        let info = if let Some(m) = meta {
            format!(
                "{} scanned · {} files · {} dirs · Sort: {} · [?] help",
                format_size(m.total_size),
                m.total_files,
                m.total_dirs,
                sort_label,
            )
        } else {
            format!("Sort: {} · [?] help", sort_label)
        };

        let line = Line::from(Span::styled(info, self.app.theme.status_bar));
        buf.set_line(area.x, area.y, &line, area.width);
    }
}
```

- [ ] **Step 3: Write the tree view rendering**

Create `src/tui/tree.rs`:

```rust
use crate::format::format_size;
use crate::tui::app::{App, VisibleItem};
use crate::tui::icons;
use crate::tui::theme::Theme;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Widget,
};

pub struct TreeView<'a> {
    app: &'a App,
}

impl<'a> TreeView<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for TreeView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let height = area.height as usize;

        // Adjust scroll to keep cursor visible
        let scroll = if self.app.cursor < self.app.scroll_offset {
            self.app.cursor
        } else if self.app.cursor >= self.app.scroll_offset + height {
            self.app.cursor - height + 1
        } else {
            self.app.scroll_offset
        };

        for (i, idx) in (scroll..self.app.visible_items.len())
            .take(height)
            .enumerate()
        {
            let item = &self.app.visible_items[idx];
            let y = area.y + i as u16;
            let is_selected = idx == self.app.cursor;
            let is_marked = self.app.marked.contains(&idx);

            render_item(
                buf,
                area.x,
                y,
                area.width,
                item,
                is_selected,
                is_marked,
                &self.app.theme,
            );
        }
    }
}

fn render_item(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    width: u16,
    item: &VisibleItem,
    is_selected: bool,
    is_marked: bool,
    theme: &Theme,
) {
    let indent = "  ".repeat(item.depth);
    let icon = icons::icon_for_name(&item.node.name)
        .unwrap_or_else(|| icons::icon_for(&item.node.name, item.node.is_dir, item.is_expanded));

    let mark = if is_marked { "● " } else { "  " };

    let tree_prefix = if item.node.is_dir {
        if item.is_expanded { "▼ " } else { "▶ " }
    } else {
        "  "
    };

    // Size column (right-aligned, 10 chars)
    let size_str = format_size(item.node.disk_size);

    // Percentage bar (8 chars)
    let ratio = if item.parent_size > 0 {
        item.node.disk_size as f64 / item.parent_size as f64
    } else {
        0.0
    };
    let bar = render_bar(ratio, 8);

    // Item count for directories
    let count_str = if item.node.is_dir && item.node.file_count > 0 {
        format!(" ({})", item.node.file_count)
    } else {
        String::new()
    };

    // Build the line
    let name_style = if is_selected {
        theme.selected
    } else if item.node.is_dir {
        theme.dir
    } else {
        theme.file_style(item.node.disk_size, item.parent_size)
    };

    let base_style = if is_selected {
        theme.selected
    } else {
        Style::default()
    };

    // Calculate available width for name
    let prefix_len = indent.len() + mark.len() + tree_prefix.len() + icon.len();
    let suffix_len = size_str.len() + 1 + bar.len() + 1 + count_str.len();
    let name_width = (width as usize).saturating_sub(prefix_len + suffix_len);

    let name = if item.node.name.len() > name_width {
        format!("{}…", &item.node.name[..name_width.saturating_sub(1)])
    } else {
        format!("{:<width$}", item.node.name, width = name_width)
    };

    let spans = vec![
        Span::styled(mark, base_style),
        Span::styled(indent, base_style),
        Span::styled(tree_prefix, base_style),
        Span::styled(icon, name_style),
        Span::styled(name, name_style),
        Span::styled(count_str, base_style),
        Span::styled(format!(" {} ", size_str), base_style),
        Span::styled(bar, base_style),
    ];

    let line = Line::from(spans);
    buf.set_line(x, y, &line, width);
}

fn render_bar(ratio: f64, width: usize) -> String {
    let filled = (ratio * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}
```

- [ ] **Step 4: Write the detail pane**

Create `src/tui/detail.rs`:

```rust
use crate::format::format_size;
use crate::tui::app::App;
use crate::tui::icons;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Modifier,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

pub struct DetailPane<'a> {
    app: &'a App,
}

impl<'a> DetailPane<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for DetailPane<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Details ")
            .borders(Borders::ALL)
            .border_style(self.app.theme.border);

        let inner = block.inner(area);
        block.render(area, buf);

        let item = match self.app.visible_items.get(self.app.cursor) {
            Some(item) => item,
            None => return,
        };

        let icon = icons::icon_for(&item.node.name, item.node.is_dir, item.is_expanded);
        let mut lines = vec![
            Line::from(Span::styled(
                format!("{}{}", icon, item.node.name),
                self.app.theme.header,
            )),
            Line::from("─".repeat(inner.width as usize)),
            Line::from(format!("Size: {}", format_size(item.node.disk_size))),
            Line::from(format!(
                "Logical: {}",
                format_size(item.node.logical_size)
            )),
        ];

        if item.node.is_dir {
            lines.push(Line::from(format!("Files: {}", item.node.file_count)));
        }

        if let Some(ts) = item.node.modified_at {
            lines.push(Line::from(format!("Modified: {}", format_timestamp(ts))));
        }
        if let Some(ts) = item.node.created_at {
            lines.push(Line::from(format!("Created: {}", format_timestamp(ts))));
        }

        if let Some(ext) = &item.node.extension {
            lines.push(Line::from(format!("Type: .{}", ext)));
        }

        // Proportion of parent
        if item.parent_size > 0 {
            let pct = (item.node.disk_size as f64 / item.parent_size as f64) * 100.0;
            lines.push(Line::from(""));
            lines.push(Line::from(format!("{:.1}% of parent", pct)));
        }

        let paragraph = Paragraph::new(lines);
        paragraph.render(inner, buf);
    }
}

fn format_timestamp(ts: i64) -> String {
    // Simple date formatting from unix timestamp
    let secs = ts as u64;
    let days = secs / 86400;
    let years = 1970 + days / 365;
    let remaining_days = days % 365;
    let months = remaining_days / 30 + 1;
    let day = remaining_days % 30 + 1;
    format!("{}-{:02}-{:02}", years, months, day)
}
```

- [ ] **Step 5: Update tui/mod.rs with full render function**

Replace `src/tui/mod.rs`:

```rust
pub mod app;
pub mod detail;
pub mod event;
pub mod icons;
pub mod statusbar;
pub mod tabs;
pub mod theme;
pub mod tree;

use app::App;
use ratatui::{
    layout::{Constraint, Direction, Layout},
    Frame,
};

/// Top-level render function.
pub fn render(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tab bar
            Constraint::Min(1),   // main content
            Constraint::Length(1), // status bar
        ])
        .split(frame.area());

    // Tab bar
    let tab_bar = tabs::TabBar::new(app.view, &app.theme);
    frame.render_widget(tab_bar, chunks[0]);

    // Main content
    if app.show_detail {
        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(65), // tree
                Constraint::Percentage(35), // detail
            ])
            .split(chunks[1]);

        let tree_view = tree::TreeView::new(app);
        frame.render_widget(tree_view, main_chunks[0]);

        let detail_pane = detail::DetailPane::new(app);
        frame.render_widget(detail_pane, main_chunks[1]);
    } else {
        let tree_view = tree::TreeView::new(app);
        frame.render_widget(tree_view, chunks[1]);
    }

    // Status bar
    let status = statusbar::StatusBar::new(app);
    frame.render_widget(status, chunks[2]);

    // Help overlay
    if app.show_help {
        render_help(frame);
    }

    // Confirm dialog overlay
    if let Some(ref confirm) = app.confirm_delete {
        render_confirm(frame, confirm, &app.theme);
    }
}

fn render_help(frame: &mut Frame) {
    use ratatui::widgets::{Block, Borders, Clear, Paragraph};

    let area = centered_rect(60, 70, frame.area());
    frame.render_widget(Clear, area);

    let help_text = vec![
        "Navigation:",
        "  j/↓  Move down        k/↑  Move up",
        "  l/→  Expand/enter     h/←  Collapse/parent",
        "  g    Jump to top      G    Jump to bottom",
        "  ^d   Page down        ^u   Page up",
        "",
        "Actions:",
        "  d     Delete (confirm)  Space  Mark/unmark",
        "  v     Invert marks      a      Mark all",
        "",
        "Views:",
        "  Tab    Next view       S-Tab  Previous view",
        "  s      Cycle sort      i      Toggle details",
        "  /      Search          ?      This help",
        "  q      Quit            r      Refresh",
    ];

    let paragraph = Paragraph::new(help_text.join("\n"))
        .block(Block::default().title(" Help ").borders(Borders::ALL));
    frame.render_widget(paragraph, area);
}

fn render_confirm(frame: &mut Frame, confirm: &app::DeleteConfirm, theme: &theme::Theme) {
    use crate::format::format_size;
    use ratatui::widgets::{Block, Borders, Clear, Paragraph};

    let area = centered_rect(45, 30, frame.area());
    frame.render_widget(Clear, area);

    let kind = if confirm.is_dir { "directory" } else { "file" };
    let text = format!(
        "Delete {}?\n\n{}\nSize: {}\n\n[d] Delete  [t] Trash  [Esc] Cancel",
        kind,
        confirm.name,
        format_size(confirm.size),
    );

    let paragraph = Paragraph::new(text)
        .block(Block::default().title(" Confirm Delete ").borders(Borders::ALL));
    frame.render_widget(paragraph, area);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: ratatui::layout::Rect) -> ratatui::layout::Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
```

- [ ] **Step 6: Build and test**

Run: `cargo build`
Expected: Compiles successfully.

Run: `cargo run -- scan /tmp && cargo run -- tui /tmp`
Expected: TUI shows tab bar, tree with icons/sizes/bars, detail pane, status bar. Navigate with j/k, expand with l, quit with q.

- [ ] **Step 7: Commit**

```bash
git add src/tui/
git commit -m "feat: TUI tree view with icons, size bars, detail pane, tabs, and status bar"
```

---

## Task 10: TUI List Views (Large Files, Recent, Old, Dev Artifacts)

**Files:**
- Create: `src/tui/views.rs`
- Modify: `src/tui/mod.rs`
- Modify: `src/tui/app.rs`

- [ ] **Step 1: Write view data loading**

Create `src/tui/views.rs`:

```rust
use crate::cache::reader::{self, FileRow, TreeNode};
use crate::format::parse_size;
use anyhow::Result;
use rusqlite::Connection;

/// Load data for the current view.
pub fn load_view_data(
    conn: &Connection,
    view: crate::tui::app::View,
    config: &ViewConfig,
) -> Result<ViewData> {
    use crate::tui::app::View;

    match view {
        View::Tree => Ok(ViewData::Tree), // handled separately
        View::LargeFiles => {
            let threshold = parse_size(&config.large_file_threshold)?;
            let files = reader::query_large_files(conn, threshold, 1000)?;
            Ok(ViewData::FileList(files))
        }
        View::Recent => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs() as i64;
            let after = now - (config.recent_days as i64 * 86400);
            let files = reader::query_recent_files(conn, after, 1000)?;
            Ok(ViewData::FileList(files))
        }
        View::Old => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_secs() as i64;
            let before = now - (config.old_days as i64 * 86400);
            let files = reader::query_old_files(conn, before, 1000)?;
            Ok(ViewData::FileList(files))
        }
        View::DevArtifacts => {
            let artifacts = reader::query_dev_artifacts(conn)?;
            Ok(ViewData::DirList(artifacts))
        }
        View::Duplicates => {
            // On-demand — return empty until user triggers scan
            Ok(ViewData::FileList(Vec::new()))
        }
    }
}

pub struct ViewConfig {
    pub large_file_threshold: String,
    pub recent_days: u32,
    pub old_days: u32,
}

pub enum ViewData {
    Tree,
    FileList(Vec<FileRow>),
    DirList(Vec<TreeNode>),
}
```

- [ ] **Step 2: Update app.rs to load view data on tab switch**

Add to `handle_key` in `src/tui/app.rs`, in the `KeyCode::Tab` and `KeyCode::BackTab` arms, after `app.view = app.view.next()`:

```rust
KeyCode::Tab => {
    app.view = app.view.next();
    load_current_view(app, conn);
    app.cursor = 0;
    event::request_render(false);
}
KeyCode::BackTab => {
    app.view = app.view.prev();
    load_current_view(app, conn);
    app.cursor = 0;
    event::request_render(false);
}
```

Add this function to `src/tui/app.rs`:

```rust
fn load_current_view(app: &mut App, conn: &Connection) {
    use crate::tui::views::{load_view_data, ViewConfig, ViewData};

    let config = ViewConfig {
        large_file_threshold: "500M".into(),
        recent_days: 7,
        old_days: 365,
    };

    match load_view_data(conn, app.view, &config) {
        Ok(ViewData::FileList(files)) => {
            app.list_items = files;
        }
        Ok(ViewData::DirList(nodes)) => {
            // Convert to visible items for rendering
            app.list_items = nodes
                .iter()
                .map(|n| FileRow {
                    name: n.name.clone(),
                    full_path: n.name.clone(),
                    disk_size: n.disk_size,
                    logical_size: n.logical_size,
                    created_at: n.created_at,
                    modified_at: n.modified_at,
                    extension: None,
                })
                .collect();
        }
        Ok(ViewData::Tree) => {
            // Tree view uses visible_items, not list_items
        }
        Err(e) => {
            eprintln!("Error loading view: {}", e);
        }
    }
}
```

- [ ] **Step 3: Add list view rendering to mod.rs render function**

Update the main content section in `src/tui/mod.rs` `render()` to handle list views:

```rust
// Main content - choose tree or list based on view
match app.view {
    app::View::Tree => {
        if app.show_detail {
            let main_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(65),
                    Constraint::Percentage(35),
                ])
                .split(chunks[1]);

            let tree_view = tree::TreeView::new(app);
            frame.render_widget(tree_view, main_chunks[0]);
            let detail_pane = detail::DetailPane::new(app);
            frame.render_widget(detail_pane, main_chunks[1]);
        } else {
            let tree_view = tree::TreeView::new(app);
            frame.render_widget(tree_view, chunks[1]);
        }
    }
    _ => {
        // List view for non-tree views
        render_list_view(frame, app, chunks[1]);
    }
}
```

Add `render_list_view` function to `src/tui/mod.rs`:

```rust
fn render_list_view(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    use crate::format::format_size;
    use ratatui::text::{Line, Span};
    use ratatui::widgets::Paragraph;

    let height = area.height as usize;
    let mut lines = Vec::new();

    if app.list_items.is_empty() {
        lines.push(Line::from("  No items found for this view."));
    }

    for (i, item) in app.list_items.iter().enumerate().take(height) {
        let is_selected = i == app.cursor;
        let style = if is_selected {
            app.theme.selected
        } else {
            app.theme.file
        };

        let icon = icons::icon_for(&item.name, false, false);
        let line = Line::from(vec![
            Span::styled(format!("  {} ", icon), style),
            Span::styled(
                format!("{:<50}", item.name),
                style,
            ),
            Span::styled(
                format!("{:>10}", format_size(item.disk_size)),
                style,
            ),
            Span::styled(
                format!("  {}", item.full_path),
                app.theme.file_small,
            ),
        ]);
        lines.push(line);
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}
```

- [ ] **Step 4: Register views module**

Add `pub mod views;` to `src/tui/mod.rs`.

- [ ] **Step 5: Build and test**

Run: `cargo build`
Expected: Compiles.

Run: `cargo run -- scan --full /tmp && cargo run -- tui /tmp`
Expected: Tab cycles through views. Large Files/Recent/Old/Dev Artifacts show appropriate filtered lists.

- [ ] **Step 6: Commit**

```bash
git add src/tui/views.rs src/tui/mod.rs src/tui/app.rs
git commit -m "feat: TUI list views for Large Files, Recent, Old, and Dev Artifacts"
```

---

## Task 11: TUI Delete with Trash Support

**Files:**
- Create: `src/delete/mod.rs`
- Create: `src/delete/trash.rs`
- Modify: `src/tui/app.rs`

- [ ] **Step 1: Write the delete module**

Create `src/delete/trash.rs`:

```rust
use crate::scanner::safety::is_dangerous_path;
use anyhow::Result;
use std::path::Path;

/// Result of a delete operation.
pub struct DeleteResult {
    pub path: String,
    pub size_freed: u64,
    pub success: bool,
    pub error: Option<String>,
}

/// Move a file or directory to macOS Trash (recoverable).
pub fn move_to_trash(path: &Path) -> Result<DeleteResult> {
    if is_dangerous_path(path) {
        return Ok(DeleteResult {
            path: path.to_string_lossy().to_string(),
            size_freed: 0,
            success: false,
            error: Some("Refusing to delete dangerous path".into()),
        });
    }

    let meta = path.symlink_metadata()?;
    let size = if meta.is_dir() {
        dir_size(path)
    } else {
        std::os::unix::fs::MetadataExt::blocks(&meta) * 512
    };

    match trash::delete(path) {
        Ok(()) => Ok(DeleteResult {
            path: path.to_string_lossy().to_string(),
            size_freed: size,
            success: true,
            error: None,
        }),
        Err(e) => Ok(DeleteResult {
            path: path.to_string_lossy().to_string(),
            size_freed: 0,
            success: false,
            error: Some(e.to_string()),
        }),
    }
}

/// Permanently delete a file or directory.
pub fn delete_permanent(path: &Path) -> Result<DeleteResult> {
    if is_dangerous_path(path) {
        return Ok(DeleteResult {
            path: path.to_string_lossy().to_string(),
            size_freed: 0,
            success: false,
            error: Some("Refusing to delete dangerous path".into()),
        });
    }

    let meta = path.symlink_metadata()?;
    let size = if meta.is_dir() {
        dir_size(path)
    } else {
        std::os::unix::fs::MetadataExt::blocks(&meta) * 512
    };

    let result = if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };

    match result {
        Ok(()) => Ok(DeleteResult {
            path: path.to_string_lossy().to_string(),
            size_freed: size,
            success: true,
            error: None,
        }),
        Err(e) => Ok(DeleteResult {
            path: path.to_string_lossy().to_string(),
            size_freed: 0,
            success: false,
            error: Some(e.to_string()),
        }),
    }
}

fn dir_size(path: &Path) -> u64 {
    jwalk::WalkDir::new(path)
        .skip_hidden(false)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.path().symlink_metadata().ok())
        .filter(|m| !m.is_dir())
        .map(|m| std::os::unix::fs::MetadataExt::blocks(&m) * 512)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_dangerous_path_blocked() {
        let result = move_to_trash(Path::new("/System")).unwrap();
        assert!(!result.success);
        assert!(result.error.unwrap().contains("dangerous"));
    }

    #[test]
    fn test_delete_permanent_file() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("test_delete.txt");
        fs::write(&file, "delete me").unwrap();

        let result = delete_permanent(&file).unwrap();
        assert!(result.success);
        assert!(!file.exists());
    }
}
```

- [ ] **Step 2: Create delete mod.rs**

Create `src/delete/mod.rs`:

```rust
pub mod trash;
```

- [ ] **Step 3: Wire delete into TUI confirm handler**

In `src/tui/app.rs`, update the confirm dialog key handler:

```rust
KeyCode::Char('y') | KeyCode::Char('d') => {
    if let Some(ref confirm) = app.confirm_delete {
        let path = std::path::Path::new(&confirm.path);
        let result = crate::delete::trash::delete_permanent(path);
        // TODO: show result notification, refresh tree
    }
    app.confirm_delete = None;
    app.rebuild_visible(conn);
}
KeyCode::Char('t') => {
    if let Some(ref confirm) = app.confirm_delete {
        let path = std::path::Path::new(&confirm.path);
        let result = crate::delete::trash::move_to_trash(path);
        // TODO: show result notification, refresh tree
    }
    app.confirm_delete = None;
    app.rebuild_visible(conn);
}
```

- [ ] **Step 4: Add delete module to lib.rs**

Add `pub mod delete;` to `src/lib.rs`.

- [ ] **Step 5: Run tests**

Run: `cargo test`
Expected: All tests pass including delete tests.

- [ ] **Step 6: Commit**

```bash
git add src/delete/ src/lib.rs
git commit -m "feat: safe deletion with trash support and dangerous path protection"
```

---

## Task 12: TUI Fuzzy Search

**Files:**
- Create: `src/tui/search.rs`
- Modify: `src/tui/app.rs`

- [ ] **Step 1: Write fuzzy search module**

Create `src/tui/search.rs`:

```rust
use nucleo::{Config, Nucleo, Utf32String};

pub struct SearchState {
    pub active: bool,
    pub query: String,
    pub matcher: Nucleo<String>,
    pub results: Vec<SearchResult>,
}

pub struct SearchResult {
    pub index: usize, // index into visible_items
    pub score: u32,
}

impl SearchState {
    pub fn new() -> Self {
        let config = Config::DEFAULT.match_paths();
        let matcher = Nucleo::new(config, std::sync::Arc::new(|| {}), None, 1);
        Self {
            active: false,
            query: String::new(),
            matcher,
            results: Vec::new(),
        }
    }

    pub fn activate(&mut self) {
        self.active = true;
        self.query.clear();
        self.results.clear();
    }

    pub fn deactivate(&mut self) {
        self.active = false;
        self.query.clear();
        self.results.clear();
    }

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
    }

    pub fn pop_char(&mut self) {
        self.query.pop();
    }

    /// Filter visible items by the current query.
    pub fn filter(&mut self, items: &[(usize, &str)]) {
        self.results.clear();

        if self.query.is_empty() {
            return;
        }

        // Simple substring match as fallback (nucleo integration can be refined later)
        let query_lower = self.query.to_lowercase();
        for (idx, name) in items {
            if name.to_lowercase().contains(&query_lower) {
                self.results.push(SearchResult {
                    index: *idx,
                    score: 100,
                });
            }
        }
    }
}
```

- [ ] **Step 2: Add search to App state and key handler**

Add `pub search: SearchState` to `App::new()`:

```rust
pub fn new(theme_name: &str) -> Self {
    Self {
        // ... existing fields ...
        search: crate::tui::search::SearchState::new(),
    }
}
```

Add search key handling at the top of `handle_key`:

```rust
// Handle search input
if app.search.active {
    match key.code {
        KeyCode::Esc => {
            app.search.deactivate();
        }
        KeyCode::Enter => {
            // Jump to first result
            if let Some(result) = app.search.results.first() {
                app.cursor = result.index;
            }
            app.search.deactivate();
        }
        KeyCode::Backspace => {
            app.search.pop_char();
            let items: Vec<(usize, &str)> = app
                .visible_items
                .iter()
                .enumerate()
                .map(|(i, v)| (i, v.node.name.as_str()))
                .collect();
            app.search.filter(&items);
        }
        KeyCode::Char(c) => {
            app.search.push_char(c);
            let items: Vec<(usize, &str)> = app
                .visible_items
                .iter()
                .enumerate()
                .map(|(i, v)| (i, v.node.name.as_str()))
                .collect();
            app.search.filter(&items);
            // Auto-jump to first match
            if let Some(result) = app.search.results.first() {
                app.cursor = result.index;
            }
        }
        _ => {}
    }
    return;
}
```

Add `/` key to activate search:

```rust
KeyCode::Char('/') => {
    app.search.activate();
}
```

- [ ] **Step 3: Build and test**

Run: `cargo build`
Expected: Compiles.

Run: `cargo run -- tui /tmp`
Expected: Press `/`, type to filter, Enter to jump, Esc to cancel.

- [ ] **Step 4: Commit**

```bash
git add src/tui/search.rs src/tui/app.rs src/tui/mod.rs
git commit -m "feat: fuzzy search in TUI with / key activation"
```

---

## Task 13: Duplicate Detection (On-Demand)

**Files:**
- Modify: `src/cache/reader.rs`
- Modify: `src/tui/app.rs`

- [ ] **Step 1: Add duplicate detection to cache reader**

Add to `src/cache/reader.rs`:

```rust
/// Find duplicate file candidates (same size), then hash to confirm.
/// Returns groups of files that are true duplicates.
pub fn find_duplicates(
    conn: &Connection,
    on_progress: impl Fn(usize, usize), // (hashed, total_candidates)
) -> Result<Vec<DuplicateGroup>> {
    // Step 1: Find files with duplicate sizes (at least 2 files with same size)
    let mut stmt = conn.prepare(
        "SELECT f1.id, f1.name, f1.disk_size, f1.dir_id
         FROM files f1
         WHERE f1.disk_size > 0
           AND (SELECT COUNT(*) FROM files f2 WHERE f2.disk_size = f1.disk_size AND f2.id != f1.id) > 0
         ORDER BY f1.disk_size DESC",
    )?;

    let candidates: Vec<(i64, String, u64, i64)> = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? as u64,
                row.get::<_, i64>(3)?,
            ))
        })?
        .filter_map(|r| r.ok())
        .collect();

    let total = candidates.len();
    let mut hashed = 0;

    // Step 2: Hash each candidate and update the DB
    for (id, _name, _size, dir_id) in &candidates {
        let path = format!("{}/{}", reconstruct_path(conn, *dir_id)?, _name);
        if let Ok(hash) = hash_file(std::path::Path::new(&path)) {
            conn.execute(
                "UPDATE files SET content_hash = ?1 WHERE id = ?2",
                rusqlite::params![hash, id],
            )?;
        }
        hashed += 1;
        on_progress(hashed, total);
    }

    // Step 3: Group by hash
    let mut group_stmt = conn.prepare(
        "SELECT content_hash, GROUP_CONCAT(id) as ids, COUNT(*) as cnt, disk_size
         FROM files
         WHERE content_hash IS NOT NULL
         GROUP BY content_hash
         HAVING cnt > 1
         ORDER BY disk_size DESC",
    )?;

    let groups: Vec<DuplicateGroup> = group_stmt
        .query_map([], |row| {
            let hash: String = row.get(0)?;
            let ids_str: String = row.get(1)?;
            let count: i64 = row.get(2)?;
            let size: i64 = row.get(3)?;

            let file_ids: Vec<i64> = ids_str
                .split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect();

            Ok(DuplicateGroup {
                hash,
                size: size as u64,
                count: count as usize,
                file_ids,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(groups)
}

#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    pub hash: String,
    pub size: u64,
    pub count: usize,
    pub file_ids: Vec<i64>,
}

fn hash_file(path: &std::path::Path) -> Result<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 65536]; // 64KB buffer

    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    Ok(hasher.finalize().to_hex().to_string())
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/cache/reader.rs
git commit -m "feat: on-demand duplicate detection with blake3 hashing"
```

---

## Task 14: Polish and Final Integration

**Files:**
- Create: `themes/dark.toml`
- Modify: `src/main.rs` (add progress bar to scan)
- Modify: `src/tui/app.rs` (scroll fix)

- [ ] **Step 1: Add progress bar to scan command**

Update the `run_scan` function in `src/main.rs` to use `indicatif`:

```rust
async fn run_scan(
    path: PathBuf,
    full: bool,
    dirs_only: bool,
    min_size: &str,
) -> anyhow::Result<()> {
    use diskcopilot::cache;
    use diskcopilot::format::{format_size, parse_size};
    use diskcopilot::scanner::walker::{ScanConfig, ScanProgress, scan_directory};
    use indicatif::{ProgressBar, ProgressStyle};
    use std::sync::Arc;

    let min_file_size = if full { 0 } else { parse_size(min_size)? };

    let config = ScanConfig {
        min_file_size,
        cache_files: !dirs_only,
        full,
    };

    let db_path = cache::db_path_for(&path)?;
    if db_path.exists() {
        std::fs::remove_file(&db_path)?;
    }

    let conn = cache::schema::open_db(&db_path)?;
    cache::schema::create_tables(&conn)?;

    let progress = Arc::new(ScanProgress::new());
    let start = Instant::now();

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );

    // Progress reporting thread
    let progress_clone = progress.clone();
    let pb_clone = pb.clone();
    let progress_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let files = progress_clone.files_found.load(Ordering::Relaxed);
            let dirs = progress_clone.dirs_found.load(Ordering::Relaxed);
            let size = progress_clone.total_size.load(Ordering::Relaxed);
            pb_clone.set_message(format!(
                "Scanning... {} files, {} dirs, {}",
                files, dirs, format_size(size)
            ));
        }
    });

    let mut writer = cache::writer::CacheWriter::new(&conn, 5000);
    scan_directory(&path, &config, &mut writer, &progress)?;
    writer.finalize()?;

    progress_handle.abort();
    pb.finish_and_clear();

    let elapsed = start.elapsed();
    cache::schema::create_indexes(&conn)?;

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;

    writer.write_meta(cache::writer::ScanMeta {
        root_path: path.to_string_lossy().to_string(),
        scanned_at: now,
        total_files: progress.files_found.load(Ordering::Relaxed),
        total_dirs: progress.dirs_found.load(Ordering::Relaxed),
        total_size: progress.total_size.load(Ordering::Relaxed),
        scan_duration_ms: elapsed.as_millis() as u64,
    })?;

    println!(
        "✓ {} files, {} dirs, {} scanned in {:.1}s",
        progress.files_found.load(Ordering::Relaxed),
        progress.dirs_found.load(Ordering::Relaxed),
        format_size(progress.total_size.load(Ordering::Relaxed)),
        elapsed.as_secs_f64(),
    );
    println!("  Cache: {}", db_path.display());

    Ok(())
}
```

- [ ] **Step 2: Create dark theme TOML**

Create `themes/dark.toml`:

```toml
[colors]
bg = "reset"
fg = "white"
dir = "blue"
file_large = "red"
file_medium = "yellow"
file_small = "dark_gray"
selected_bg = "dark_gray"
selected_fg = "white"
bar_low = "green"
bar_mid = "yellow"
bar_high = "red"
tab_active = "cyan"
tab_inactive = "dark_gray"
status = "dark_gray"
header = "cyan"
border = "dark_gray"
```

- [ ] **Step 3: Fix scroll tracking in App**

Add scroll offset update to `move_cursor` in `src/tui/app.rs`:

```rust
pub fn move_cursor(&mut self, delta: i32) {
    let len = match self.view {
        View::Tree => self.visible_items.len(),
        _ => self.list_items.len(),
    };
    let len = len.max(1);
    let new = (self.cursor as i32 + delta).clamp(0, len as i32 - 1) as usize;
    self.cursor = new;

    // Keep cursor visible (assuming ~40 rows visible)
    let visible_height = 40;
    if self.cursor < self.scroll_offset {
        self.scroll_offset = self.cursor;
    } else if self.cursor >= self.scroll_offset + visible_height {
        self.scroll_offset = self.cursor - visible_height + 1;
    }
}
```

- [ ] **Step 4: Final build and test**

Run: `cargo test`
Expected: All tests pass.

Run: `cargo build --release`
Expected: Release binary compiles.

Run: `cargo run --release -- scan ~`
Expected: Scans home directory with progress spinner, shows summary.

Run: `cargo run --release -- tui ~`
Expected: Full TUI with tree view, tab switching, navigation, detail pane, help overlay.

- [ ] **Step 5: Commit**

```bash
git add src/main.rs themes/ src/tui/app.rs
git commit -m "feat: scan progress bar, dark theme, scroll tracking polish"
```

---

## Summary

| Task | What it builds | Key files |
|------|---------------|-----------|
| 1 | Project scaffold, CLI skeleton, size formatting | `Cargo.toml`, `main.rs`, `format.rs` |
| 2 | SQLite schema with tables and deferred indexes | `cache/schema.rs` |
| 3 | Batch cache writer with dir size aggregation | `cache/writer.rs` |
| 4 | Parallel scanner with inode dedup and safety | `scanner/walker.rs`, `metadata.rs`, `safety.rs` |
| 5 | Wired scan command with integration test | `main.rs`, `lib.rs`, `tests/scanner_test.rs` |
| 6 | Cache reader with tree reconstruction and view queries | `cache/reader.rs` |
| 7 | Config module with TOML loading | `config/loader.rs` |
| 8 | TUI foundation: events, themes, icons, app shell | `tui/event.rs`, `app.rs`, `theme.rs`, `icons.rs` |
| 9 | TUI tree view with tabs, status bar, detail pane | `tui/tree.rs`, `tabs.rs`, `statusbar.rs`, `detail.rs` |
| 10 | TUI list views for all non-tree tabs | `tui/views.rs` |
| 11 | Delete with trash support and safety checks | `delete/trash.rs` |
| 12 | Fuzzy search with `/` activation | `tui/search.rs` |
| 13 | On-demand duplicate detection with blake3 | `cache/reader.rs` |
| 14 | Polish: progress bar, theme file, scroll fix | `main.rs`, `themes/dark.toml` |
