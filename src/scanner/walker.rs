use crate::cache::writer::{CacheWriter, DirEntry, FileEntry};
use crate::scanner::metadata::file_extension;
use anyhow::Result;
use jwalk::WalkDirGeneric;
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

/// Atomic scan progress counters — safe to share across threads / read from UI.
pub struct ScanProgress {
    pub(crate) files_found: AtomicU64,
    pub(crate) dirs_found: AtomicU64,
    pub(crate) total_size: AtomicU64,
}

impl ScanProgress {
    pub fn new() -> Self {
        Self {
            files_found: AtomicU64::new(0),
            dirs_found: AtomicU64::new(0),
            total_size: AtomicU64::new(0),
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
// Per-file metadata computed in parallel on jwalk's rayon thread pool.
// Stored in entry.client_state to avoid a second stat() on the main thread.
#[derive(Debug, Clone, Default)]
struct FileStat {
    logical_size: u64,
    disk_size: u64,
    created_at: Option<i64>,
    modified_at: Option<i64>,
    inode: u64,
}

pub fn scan_directory(
    root: &Path,
    config: &ScanConfig,
    writer: &mut CacheWriter<'_>,
    progress: &ScanProgress,
) -> Result<HashMap<i64, (i64, i64)>> {
    let min_file_size = config.min_file_size;
    let full = config.full;

    // Detect root filesystem device to skip cross-device mounts
    // (network shares, USB drives, Time Machine volumes)
    let root_dev: u64 = root.metadata().map(|m| m.dev()).unwrap_or(0);

    // macOS bundle extensions — treated as opaque files, not descended into
    const BUNDLE_EXTS: &[&str] = &[".app", ".framework", ".bundle", ".plugin", ".kext"];
    let is_bundle = |name: &str| BUNDLE_EXTS.iter().any(|ext| name.ends_with(ext));

    // Bundles found in process_read_dir (parallel) are collected here
    // and merged as file entries after the walk.
    // Tuple: (parent_path, name, disk_size, logical_size)
    type BundleVec = Vec<(PathBuf, String, u64, u64)>;
    let bundles: Arc<std::sync::Mutex<BundleVec>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let bundles_ref = bundles.clone();

    // Small files (below min_file_size) skipped in process_read_dir.
    // Their sizes are accumulated here so directory totals remain accurate.
    // Tuple: (parent_path, disk_size, logical_size)
    type SkippedVec = Vec<(PathBuf, u64, u64)>;
    let skipped_files: Arc<std::sync::Mutex<SkippedVec>> =
        Arc::new(std::sync::Mutex::new(Vec::new()));
    let skipped_ref = skipped_files.clone();

    // Phase 1: parallel stat via process_read_dir.
    // File metadata (stat syscall) is computed on jwalk's rayon thread pool,
    // parallelising millions of stat() calls across all cores.
    let walker = WalkDirGeneric::<((), FileStat)>::new(root)
        .skip_hidden(false)
        .follow_links(false)
        .process_read_dir(move |_depth, _path, _state, children| {
            children.retain_mut(|entry_result| {
                let Ok(entry) = entry_result else { return false };

                // Drop symlinks
                if entry.file_type.is_symlink() {
                    return false;
                }

                if entry.file_type.is_dir() {
                    let p = entry.path();

                    // APFS firmlink exclusions — prevent double-counting
                    const EXCLUDED: &[&str] = &[
                        "/System/Volumes/Data",
                        "/System/Volumes/Update/mnt1",
                        "/System/Volumes/Update/SFR/mnt1",
                    ];
                    if EXCLUDED.iter().any(|ex| p == Path::new(ex)) {
                        return false;
                    }

                    // Skip cross-device mounts (network, USB, Time Machine)
                    if root_dev != 0 {
                        if let Ok(m) = p.metadata() {
                            if m.dev() != root_dev {
                                return false;
                            }
                        }
                    }

                    // macOS bundles (.app, .framework, etc.) — compute their
                    // total size on this parallel thread, then remove from walk
                    // so jwalk doesn't descend. Collected into shared vec and
                    // merged as file entries after the walk.
                    let name = entry.file_name.to_string_lossy();
                    if is_bundle(&name) {
                        let mut total_disk: u64 = 0;
                        let mut total_logical: u64 = 0;
                        for e in jwalk::WalkDir::new(&p).skip_hidden(false).follow_links(false).into_iter().flatten() {
                            if !e.file_type().is_dir() {
                                if let Ok(m) = e.path().symlink_metadata() {
                                    total_disk += m.blocks() * 512;
                                    total_logical += m.len();
                                }
                            }
                        }
                        if let Ok(mut b) = bundles_ref.lock() {
                            b.push((
                                entry.parent_path.to_path_buf(),
                                name.into_owned(),
                                total_disk,
                                total_logical,
                            ));
                        }
                        return false; // remove from walk — don't descend
                    }
                } else {
                    // File: stat() on this parallel thread
                    let stat = match entry.path().symlink_metadata() {
                        Ok(m) => {
                            #[cfg(target_os = "macos")]
                            let created_at = {
                                use std::os::darwin::fs::MetadataExt as DarwinExt;
                                Some(m.st_birthtime())
                            };
                            #[cfg(not(target_os = "macos"))]
                            let created_at = Some(m.ctime());

                            FileStat {
                                logical_size: m.len(),
                                disk_size: m.blocks() * 512,
                                created_at,
                                modified_at: Some(m.mtime()),
                                inode: m.ino(),
                            }
                        }
                        Err(_) => return false,
                    };

                    // Skip small files from DB but track their sizes for dir totals
                    if !full && stat.logical_size < min_file_size {
                        if let Ok(mut s) = skipped_ref.lock() {
                            s.push((entry.parent_path.to_path_buf(), stat.disk_size, stat.logical_size));
                        }
                        return false;
                    }

                    entry.client_state = stat;
                }

                true
            });
        });

    // Phase 2: drain walker — buffer dirs in memory, write files inline.
    // Dir records are stored in a Vec (not written to DB yet) so we can
    // prune the 66% that are empty leaves before hitting SQLite.
    struct DirRecord {
        id: i64,
        parent_idx: Option<usize>, // index into dir_records
        name: String,
    }
    let mut dir_records: Vec<DirRecord> = Vec::new();
    let mut path_to_idx: HashMap<PathBuf, usize> = HashMap::new();
    let mut path_to_dir_id: HashMap<PathBuf, i64> = HashMap::new();
    let mut dir_id_counter: i64 = 0;
    let mut seen_inodes: HashSet<u64> = HashSet::new();
    let mut file_id_counter: i64 = 0;

    // Track which dirs contain files (directly or via ancestors)
    let mut needed_dirs: HashSet<usize> = HashSet::new();

    for entry in walker.into_iter().filter_map(|e| e.ok()) {
        if entry.file_type().is_dir() {
            let path = entry.path();
            dir_id_counter += 1;
            let dir_id = dir_id_counter;
            let idx = dir_records.len();

            let parent_idx = path.parent().and_then(|p| path_to_idx.get(p)).copied();

            let name = if parent_idx.is_none() {
                path.to_string_lossy().into_owned()
            } else {
                entry.file_name().to_string_lossy().into_owned()
            };

            path_to_dir_id.insert(path.clone(), dir_id);
            path_to_idx.insert(path, idx);
            dir_records.push(DirRecord { id: dir_id, parent_idx, name });

            progress.dirs_found.fetch_add(1, Ordering::Relaxed);
        } else {
            let stat = &entry.client_state;

            if !seen_inodes.insert(stat.inode) {
                continue;
            }

            let parent_path = entry.parent_path();
            let dir_id = match path_to_dir_id.get(parent_path) {
                Some(&id) => id,
                None => continue,
            };

            // Mark this dir and all ancestors as needed
            if let Some(&idx) = path_to_idx.get(parent_path) {
                let mut cur = Some(idx);
                while let Some(i) = cur {
                    if !needed_dirs.insert(i) { break; } // already marked
                    cur = dir_records[i].parent_idx;
                }
            }

            let name = entry.file_name().to_string_lossy().into_owned();
            let ext = file_extension(&name);

            file_id_counter += 1;
            writer.add_file(FileEntry {
                id: file_id_counter,
                dir_id,
                name,
                logical_size: stat.logical_size as i64,
                disk_size: stat.disk_size as i64,
                created_at: stat.created_at,
                modified_at: stat.modified_at,
                extension: ext,
                inode: Some(stat.inode as i64),
                content_hash: None,
            })?;

            progress.files_found.fetch_add(1, Ordering::Relaxed);
            progress.total_size.fetch_add(stat.disk_size, Ordering::Relaxed);
        }
    }

    // Merge bundles
    let bundles = std::mem::take(&mut *bundles.lock().unwrap());
    for (parent_path, name, disk_size, logical_size) in bundles {
        let dir_id = match path_to_dir_id.get(&parent_path) {
            Some(&id) => id,
            None => continue,
        };
        if let Some(&idx) = path_to_idx.get(&parent_path) {
            let mut cur = Some(idx);
            while let Some(i) = cur {
                if !needed_dirs.insert(i) { break; }
                cur = dir_records[i].parent_idx;
            }
        }
        let ext = file_extension(&name);
        file_id_counter += 1;
        writer.add_file(FileEntry {
            id: file_id_counter,
            dir_id,
            name,
            logical_size: logical_size as i64,
            disk_size: disk_size as i64,
            created_at: None,
            modified_at: None,
            extension: ext,
            inode: None,
            content_hash: None,
        })?;
        progress.files_found.fetch_add(1, Ordering::Relaxed);
        progress.total_size.fetch_add(disk_size, Ordering::Relaxed);
    }

    // Build skipped sizes map from the parallel-collected vec.
    let mut skipped_map: HashMap<i64, (i64, i64)> = HashMap::new();
    let skipped_vec = std::mem::take(&mut *skipped_files.lock().unwrap());
    for (parent_path, disk_size, _logical_size) in skipped_vec {
        if let Some(&dir_id) = path_to_dir_id.get(&parent_path) {
            let entry = skipped_map.entry(dir_id).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += disk_size as i64;

            // Also mark this dir and ancestors as needed
            if let Some(&idx) = path_to_idx.get(&parent_path) {
                let mut cur = Some(idx);
                while let Some(i) = cur {
                    if !needed_dirs.insert(i) { break; }
                    cur = dir_records[i].parent_idx;
                }
            }
        }
    }

    // Write only dirs that contain files (directly or via descendants)
    for (idx, d) in dir_records.iter().enumerate() {
        if !needed_dirs.contains(&idx) {
            continue;
        }
        let parent_id = d.parent_idx
            .map(|pi| dir_records[pi].id);
        writer.add_dir(DirEntry {
            id: d.id,
            parent_id,
            name: d.name.clone(),
            created_at: None,
            modified_at: None,
        })?;
    }

    Ok(skipped_map)
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
            let skipped = scan_directory(root, &config, &mut writer, &progress)?;
            writer.finalize(&skipped)?;
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
            let skipped = scan_directory(root, &config, &mut writer, &progress)?;
            writer.finalize(&skipped)?;
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
            let skipped = scan_directory(root, &config, &mut writer, &progress)?;
            writer.finalize(&skipped)?;
        }

        assert_eq!(progress.dirs(), 1, "should find 1 dir (root only)");
        assert_eq!(progress.files(), 0, "should find 0 files");

        let dir_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM dirs", [], |r| r.get(0))?;
        let file_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        // Empty dirs are pruned from DB (optimization: skip dirs with no files)
        assert_eq!(dir_count, 0);
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
            let skipped = r.as_ref().map(|s| s.clone()).unwrap_or_default();
            writer.finalize(&skipped)?;
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
            let skipped = scan_directory(root, &config, &mut writer, &progress)?;
            writer.finalize(&skipped)?;
        }

        // Only big.bin should have a file record
        let file_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        assert_eq!(file_count, 1, "only big.bin should be in files table");
        let name: String =
            conn.query_row("SELECT name FROM files LIMIT 1", [], |r| r.get(0))?;
        assert_eq!(name, "big.bin");

        // But the directory total_disk_size should include BOTH files
        let dir_size: i64 = conn.query_row(
            "SELECT total_disk_size FROM dirs WHERE parent_id IS NULL",
            [],
            |r| r.get(0),
        )?;
        // small.txt is 4 bytes, big.bin is 2 MiB — dir size must be > 2 MiB
        // (it includes the small file's on-disk allocation)
        assert!(
            dir_size > 2 * 1024 * 1024,
            "dir size ({}) should include small files that were below the threshold",
            dir_size
        );

        Ok(())
    }

    #[test]
    fn test_dir_size_matches_du() -> Result<()> {
        use std::os::unix::fs::MetadataExt;

        let tmp = tempfile::tempdir()?;
        let root = tmp.path();

        // Create a tree with mixed file sizes
        let sub = root.join("subdir");
        std::fs::create_dir(&sub)?;
        std::fs::write(root.join("a.txt"), vec![0u8; 100])?;         // tiny
        std::fs::write(root.join("b.bin"), vec![0u8; 500_000])?;     // medium
        std::fs::write(sub.join("c.dat"), vec![0u8; 1_500_000])?;    // large
        std::fs::write(sub.join("d.log"), vec![0u8; 200])?;          // tiny

        // Calculate expected total using stat (same as du)
        let expected: u64 = jwalk::WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| e.path().symlink_metadata().ok())
            .map(|m| m.blocks() * 512)
            .sum();

        // Scan with default mode (>= 1MB threshold)
        let mut conn = make_conn();
        let progress = ScanProgress::new();
        let config = ScanConfig {
            full: false,
            min_file_size: 1024 * 1024,
        };
        {
            let mut writer = CacheWriter::new(&mut conn, 1000);
            let skipped = scan_directory(root, &config, &mut writer, &progress)?;
            writer.finalize(&skipped)?;
        }

        let scanned_size: i64 = conn.query_row(
            "SELECT total_disk_size FROM dirs WHERE parent_id IS NULL",
            [],
            |r| r.get(0),
        )?;

        assert_eq!(
            scanned_size as u64, expected,
            "scanned dir size ({}) should match du-style total ({})",
            scanned_size, expected
        );

        Ok(())
    }

    #[test]
    fn test_symlink_not_double_counted() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path();

        let real_file = root.join("real.dat");
        std::fs::write(&real_file, vec![0u8; 50_000])?;
        let link = root.join("link.dat");
        std::os::unix::fs::symlink(&real_file, &link)?;

        let mut conn = make_conn();
        let progress = ScanProgress::new();
        let config = ScanConfig {
            full: true,
            min_file_size: 0,
        };
        {
            let mut writer = CacheWriter::new(&mut conn, 1000);
            let skipped = scan_directory(root, &config, &mut writer, &progress)?;
            writer.finalize(&skipped)?;
        }

        // Only 1 file record (the real file, not the symlink)
        let file_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        assert_eq!(file_count, 1, "symlink should not create a file record");

        // Dir size should only count the real file once
        let dir_size: i64 = conn.query_row(
            "SELECT total_disk_size FROM dirs WHERE parent_id IS NULL",
            [],
            |r| r.get(0),
        )?;
        let real_size = real_file.symlink_metadata()?.blocks() * 512;
        assert_eq!(
            dir_size as u64, real_size,
            "dir size ({}) should equal single file size ({}), not double-counted",
            dir_size, real_size
        );

        Ok(())
    }
}
