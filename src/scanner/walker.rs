use crate::cache::writer::{CacheWriter, DirEntry, FileEntry};
use crate::scanner::metadata::{extract_metadata, file_extension};
use anyhow::Result;
use jwalk::WalkDir;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

/// Atomic scan progress counters — safe to share across threads / read from UI.
pub struct ScanProgress {
    pub files_found: Arc<AtomicU64>,
    pub dirs_found: Arc<AtomicU64>,
    pub total_size: Arc<AtomicU64>,
}

impl ScanProgress {
    pub fn new() -> Self {
        Self {
            files_found: Arc::new(AtomicU64::new(0)),
            dirs_found: Arc::new(AtomicU64::new(0)),
            total_size: Arc::new(AtomicU64::new(0)),
        }
    }

    pub fn files(&self) -> u64 {
        self.files_found.load(Ordering::Relaxed)
    }

    pub fn dirs(&self) -> u64 {
        self.dirs_found.load(Ordering::Relaxed)
    }

    pub fn size(&self) -> u64 {
        self.total_size.load(Ordering::Relaxed)
    }
}

impl Default for ScanProgress {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration controlling what gets stored in the cache.
pub struct ScanConfig {
    /// Minimum file size (in bytes) to cache. Files smaller than this are
    /// skipped unless `full` is true. Defaults to 1 MiB.
    pub min_file_size: u64,
    /// When true, apply no size filter (equivalent to min_file_size=0).
    pub full: bool,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            min_file_size: 1024 * 1024, // 1 MiB
            full: false,
        }
    }
}

// ---------------------------------------------------------------------------
// scan_directory
// ---------------------------------------------------------------------------

/// Walk `root` in parallel, writing directories and qualifying files to
/// `writer`.  Progress counters are updated atomically as entries are
/// discovered.
///
/// Design notes:
/// - jwalk streams entries in parallel using rayon; we collect them all then
///   process in a single thread to keep SQLite writes on one thread.
/// - Hard-linked files are deduplicated by inode (per-device, but we don't
///   span device boundaries here).
/// - Symlinks are skipped (`follow_links(false)` is the default).
/// - The caller is responsible for calling `writer.finalize()` afterward.
pub fn scan_directory(
    root: &Path,
    config: &ScanConfig,
    writer: &mut CacheWriter<'_>,
    progress: &ScanProgress,
) -> Result<()> {
    // Collect all entries from the parallel walk first.
    let entries: Vec<_> = WalkDir::new(root)
        .skip_hidden(false)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .collect();

    // ------------------------------------------------------------------
    // First pass: assign directory IDs in depth order so parents always
    // get a lower ID than their children.
    // ------------------------------------------------------------------
    let mut dir_id_counter: i64 = 0;
    // Maps canonical PathBuf → dir_id
    let mut path_to_dir_id: HashMap<PathBuf, i64> = HashMap::new();

    // First pass: directories only
    for entry in &entries {
        let file_type = entry.file_type();
        if !file_type.is_dir() {
            continue;
        }
        let path = entry.path();
        let meta = match extract_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        dir_id_counter += 1;
        let dir_id = dir_id_counter;
        path_to_dir_id.insert(path.clone(), dir_id);

        // Find parent dir_id
        let parent_id = path
            .parent()
            .and_then(|p| path_to_dir_id.get(p))
            .copied();

        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());

        writer.add_dir(DirEntry {
            id: dir_id,
            parent_id,
            name,
            created_at: meta.created_at,
            modified_at: meta.modified_at,
        })?;

        progress.dirs_found.fetch_add(1, Ordering::Relaxed);
    }

    // ------------------------------------------------------------------
    // Second pass: files (non-dirs, non-symlinks)
    // ------------------------------------------------------------------
    let mut seen_inodes: HashSet<u64> = HashSet::new();
    let mut file_id_counter: i64 = 0;

    for entry in &entries {
        let file_type = entry.file_type();

        // Skip directories (already handled) and symlinks
        if file_type.is_dir() || file_type.is_symlink() {
            continue;
        }

        let path = entry.path();
        let meta = match extract_metadata(&path) {
            Ok(m) => m,
            Err(_) => continue,
        };

        // Deduplicate hard links by inode
        if !seen_inodes.insert(meta.inode) {
            continue;
        }

        // Size filter (skip if not full mode and below threshold)
        if !config.full && meta.logical_size < config.min_file_size {
            continue;
        }

        // Find the containing directory
        let parent_path = match path.parent() {
            Some(p) => p,
            None => continue,
        };
        let dir_id = match path_to_dir_id.get(parent_path) {
            Some(&id) => id,
            None => continue, // parent wasn't recorded (permission error, etc.)
        };

        let name = entry
            .file_name()
            .to_string_lossy()
            .into_owned();

        let ext = file_extension(&name);

        file_id_counter += 1;

        writer.add_file(FileEntry {
            id: file_id_counter,
            dir_id,
            name,
            logical_size: meta.logical_size as i64,
            disk_size: meta.disk_size as i64,
            created_at: meta.created_at,
            modified_at: meta.modified_at,
            extension: ext,
            inode: Some(meta.inode as i64),
            content_hash: None,
        })?;

        progress.files_found.fetch_add(1, Ordering::Relaxed);
        progress
            .total_size
            .fetch_add(meta.disk_size, Ordering::Relaxed);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::schema::{create_tables, open_memory_db};

    fn make_conn() -> rusqlite::Connection {
        let conn = open_memory_db().unwrap();
        create_tables(&conn).unwrap();
        conn
    }

    #[test]
    fn test_scan_simple_tree() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path();

        // root/
        //   file_a.txt
        //   sub/
        //     file_b.txt
        std::fs::write(root.join("file_a.txt"), b"hello world")?;
        let sub = root.join("sub");
        std::fs::create_dir(&sub)?;
        std::fs::write(sub.join("file_b.txt"), b"world")?;

        let mut conn = make_conn();
        let progress = ScanProgress::new();
        let config = ScanConfig {
            full: true,
            min_file_size: 0,
        };
        {
            let mut writer = CacheWriter::new(&mut conn, 1000);
            scan_directory(root, &config, &mut writer, &progress)?;
            writer.finalize()?;
        }

        assert_eq!(progress.files(), 2, "should find 2 files");
        assert_eq!(progress.dirs(), 2, "should find 2 dirs (root + sub)");

        let file_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        let dir_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM dirs", [], |r| r.get(0))?;
        assert_eq!(file_count, 2);
        assert_eq!(dir_count, 2);

        Ok(())
    }

    #[test]
    fn test_symlinks_skipped() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path();

        let target = root.join("real.txt");
        std::fs::write(&target, b"content")?;
        let link = root.join("link.txt");
        std::os::unix::fs::symlink(&target, &link)?;

        let mut conn = make_conn();
        let progress = ScanProgress::new();
        let config = ScanConfig {
            full: true,
            min_file_size: 0,
        };
        {
            let mut writer = CacheWriter::new(&mut conn, 1000);
            scan_directory(root, &config, &mut writer, &progress)?;
            writer.finalize()?;
        }

        // Only real.txt should be indexed (link is skipped)
        assert_eq!(progress.files(), 1);
        Ok(())
    }

    #[test]
    fn test_scan_empty_directory() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path();

        let mut conn = make_conn();
        let progress = ScanProgress::new();
        let config = ScanConfig {
            full: true,
            min_file_size: 0,
        };
        {
            let mut writer = CacheWriter::new(&mut conn, 1000);
            scan_directory(root, &config, &mut writer, &progress)?;
            writer.finalize()?;
        }

        assert_eq!(progress.dirs(), 1, "should find 1 dir (root only)");
        assert_eq!(progress.files(), 0, "should find 0 files");

        let dir_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM dirs", [], |r| r.get(0))?;
        let file_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        assert_eq!(dir_count, 1);
        assert_eq!(file_count, 0);

        Ok(())
    }

    #[test]
    fn test_permission_denied_handling() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir()?;
        let root = tmp.path();

        // Create an accessible file
        std::fs::write(root.join("accessible.txt"), b"hello")?;

        // Create a subdirectory with no permissions
        let forbidden = root.join("forbidden");
        std::fs::create_dir(&forbidden)?;
        std::fs::write(forbidden.join("secret.txt"), b"hidden")?;
        std::fs::set_permissions(&forbidden, std::fs::Permissions::from_mode(0o000))?;

        let mut conn = make_conn();
        let progress = ScanProgress::new();
        let config = ScanConfig {
            full: true,
            min_file_size: 0,
        };
        let result = {
            let mut writer = CacheWriter::new(&mut conn, 1000);
            let r = scan_directory(root, &config, &mut writer, &progress);
            writer.finalize()?;
            r
        };

        // The scan should complete without error (skips the unreadable dir)
        assert!(result.is_ok(), "scan should complete without error even with permission-denied dirs");

        // Restore permissions so the tempdir can be cleaned up
        std::fs::set_permissions(&forbidden, std::fs::Permissions::from_mode(0o755))?;

        Ok(())
    }

    #[test]
    fn test_size_filter() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path();

        // Small file (< 1 MiB)
        std::fs::write(root.join("small.txt"), b"tiny")?;
        // Large file (> 1 MiB)
        let big_data = vec![0u8; 2 * 1024 * 1024];
        std::fs::write(root.join("big.bin"), &big_data)?;

        let mut conn = make_conn();
        let progress = ScanProgress::new();
        let config = ScanConfig {
            full: false,
            min_file_size: 1024 * 1024,
        };
        {
            let mut writer = CacheWriter::new(&mut conn, 1000);
            scan_directory(root, &config, &mut writer, &progress)?;
            writer.finalize()?;
        }

        // Only big.bin passes the filter
        assert_eq!(progress.files(), 1);
        let name: String =
            conn.query_row("SELECT name FROM files LIMIT 1", [], |r| r.get(0))?;
        assert_eq!(name, "big.bin");

        Ok(())
    }
}
