//! macOS-optimized parallel scanner using getattrlistbulk(2).
//!
//! Instead of stat()-ing each file individually (as jwalk does), this scanner
//! retrieves metadata for all entries in a directory with a single syscall per
//! buffer-fill, eliminating per-file stat overhead.  Benchmarks on APFS volumes
//! show a 3-6x speedup over jwalk.
//!
//! Only available on macOS.  Use `supports_bulk_attrs` to check whether the
//! target volume supports the syscall correctly (APFS and HFS+ only).

#![cfg(target_os = "macos")]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::mem::MaybeUninit;
use std::os::raw::{c_int, c_void};
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use anyhow::Result;
use crossbeam_channel::Sender;

use crate::cache::writer::{CacheWriter, DirEntry, FileEntry};
use crate::scanner::metadata::file_extension;
use crate::scanner::walker::{ScanConfig, ScanProgress};

// ── FFI constants ────────────────────────────────────────────────────────────

// <sys/attr.h> constants
const ATTR_BIT_MAP_COUNT: u16 = 5;
const ATTR_CMN_RETURNED_ATTRS: u32 = 0x80000000;
const ATTR_CMN_NAME: u32 = 0x00000001;
const ATTR_CMN_DEVID: u32 = 0x00000002;
const ATTR_CMN_OBJTYPE: u32 = 0x00000008;
const ATTR_CMN_MODTIME: u32 = 0x00000400;
const ATTR_CMN_CRTIME: u32 = 0x00000800;
const ATTR_CMN_FILEID: u32 = 0x02000000;
const ATTR_CMN_ERROR: u32 = 0x20000000;
const ATTR_FILE_LINKCOUNT: u32 = 0x00000001;
const ATTR_FILE_ALLOCSIZE: u32 = 0x00000004;
const FSOPT_PACK_INVAL_ATTRS: u64 = 0x00000002;

// <sys/vnode.h> vnode types
const VREG: u32 = 1; // regular file
const VDIR: u32 = 2; // directory
const VLNK: u32 = 5; // symlink

// ── FFI types ────────────────────────────────────────────────────────────────

#[repr(C)]
struct AttrList {
    bitmapcount: u16,
    reserved: u16,
    commonattr: u32,
    volattr: u32,
    dirattr: u32,
    fileattr: u32,
    forkattr: u32,
}

/// Returned by the kernel: which attribute groups were actually returned.
#[repr(C)]
#[derive(Copy, Clone)]
struct AttributeSet {
    commonattr: u32,
    _volattr: u32,
    _dirattr: u32,
    fileattr: u32,
    _forkattr: u32,
}

#[repr(C)]
#[derive(Copy, Clone)]
struct AttrReference {
    attr_dataoffset: i32,
    attr_length: u32,
}

extern "C" {
    fn getattrlistbulk(
        dirfd: c_int,
        attrlist: *const AttrList,
        attrbuf: *mut c_void,
        attrbufsize: usize,
        options: u64,
    ) -> c_int;
}

// ── Parsed entry ─────────────────────────────────────────────────────────────

struct BulkDirEntry {
    name: String,
    obj_type: u32,
    dev_id: i32,
    file_id: u64,
    link_count: u32,
    alloc_size: i64,
    mtime_sec: i64,
    crtime_sec: i64,
    error: u32,
}

// ── Thread-local buffer ──────────────────────────────────────────────────────

// 256 KB thread-local buffer for getattrlistbulk results.
// Reused across calls on each rayon worker thread — avoids allocating on
// every directory.
thread_local! {
    static BUF: RefCell<Vec<u8>> = RefCell::new(vec![0u8; 256 * 1024]);
}

// ── Buffer parsing ────────────────────────────────────────────────────────────

/// Read a value of type T at the current cursor position using
/// `read_unaligned` (safe for packed kernel buffers).
/// Returns None if there aren't enough bytes remaining.
#[inline]
fn read_field<T: Copy>(record: &[u8], cursor: &mut usize) -> Option<T> {
    let size = std::mem::size_of::<T>();
    if *cursor + size > record.len() {
        return None;
    }
    let val = unsafe { std::ptr::read_unaligned(record[*cursor..].as_ptr() as *const T) };
    *cursor += size;
    Some(val)
}

/// Parse one record from the getattrlistbulk output buffer.
///
/// Layout (attributes in bit-position order):
///   u32 len | attribute_set_t (RETURNED_ATTRS) |
///   [u32 ERROR] | attrreference_t (NAME) | i32 (DEVID) | u32 (OBJTYPE) |
///   timespec (MODTIME) | timespec (CRTIME) | u64 (FILEID) |
///   [u32 (LINKCOUNT) | i64 (ALLOCSIZE)]   ← file attrs, only if present
fn parse_record(record: &[u8]) -> Option<BulkDirEntry> {
    let mut cursor = 4usize; // skip u32 record length

    let returned: AttributeSet = read_field(record, &mut cursor)?;
    // ERROR is only packed when the kernel sets the ERROR bit in returned attrs
    let error: u32 = if returned.commonattr & ATTR_CMN_ERROR != 0 {
        read_field(record, &mut cursor)?
    } else {
        0
    };
    let name_ref_pos = cursor;
    let name_ref: AttrReference = read_field(record, &mut cursor)?;
    let devid: i32 = read_field(record, &mut cursor)?;
    let objtype: u32 = read_field(record, &mut cursor)?;

    // timespec: i64 seconds + i64 nanoseconds (16 bytes each on 64-bit)
    let mtime_sec: i64 = read_field(record, &mut cursor)?;
    let _mtime_nsec: i64 = read_field(record, &mut cursor)?;
    let crtime_sec: i64 = read_field(record, &mut cursor)?;
    let _crtime_nsec: i64 = read_field(record, &mut cursor)?;

    let fileid: u64 = read_field(record, &mut cursor)?;

    // File attributes are only present when the entry is a regular file
    let (link_count, alloc_size) = if returned.fileattr != 0 {
        let lc: u32 = read_field(record, &mut cursor).unwrap_or(1);
        let alloc: i64 = read_field(record, &mut cursor).unwrap_or(0);
        (lc, alloc)
    } else {
        (1u32, 0i64)
    };

    // Resolve name from attrreference_t (offset is relative to the field itself)
    let name_start = name_ref_pos + name_ref.attr_dataoffset as usize;
    let name_end = (name_start + name_ref.attr_length as usize).min(record.len());
    let name = if name_start < record.len() && name_end > name_start {
        let bytes = &record[name_start..name_end];
        let s = bytes.split(|b| *b == 0).next().unwrap_or(bytes);
        String::from_utf8_lossy(s).into_owned()
    } else {
        return None;
    };

    Some(BulkDirEntry {
        name,
        obj_type: objtype,
        dev_id: devid,
        file_id: fileid,
        link_count,
        alloc_size,
        mtime_sec,
        crtime_sec,
        error,
    })
}

// ── read_dir_bulk ─────────────────────────────────────────────────────────────

/// Read all entries in a directory using getattrlistbulk.  Returns parsed
/// entries with name, type, inode info, and physical allocation size.
fn read_dir_bulk(dir_path: &Path) -> std::io::Result<Vec<BulkDirEntry>> {
    let dir = File::open(dir_path)?;
    let fd = dir.as_raw_fd();

    let attrlist = AttrList {
        bitmapcount: ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: ATTR_CMN_RETURNED_ATTRS
            | ATTR_CMN_NAME
            | ATTR_CMN_DEVID
            | ATTR_CMN_OBJTYPE
            | ATTR_CMN_MODTIME
            | ATTR_CMN_CRTIME
            | ATTR_CMN_FILEID
            | ATTR_CMN_ERROR,
        volattr: 0,
        dirattr: 0,
        fileattr: ATTR_FILE_LINKCOUNT | ATTR_FILE_ALLOCSIZE,
        forkattr: 0,
    };

    BUF.with(|buf| {
        let mut buf = buf.borrow_mut();
        let mut entries = Vec::new();

        loop {
            let count = unsafe {
                getattrlistbulk(
                    fd,
                    &attrlist,
                    buf.as_mut_ptr() as *mut c_void,
                    buf.len(),
                    FSOPT_PACK_INVAL_ATTRS,
                )
            };
            if count < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if count == 0 {
                break;
            }

            let mut offset = 0usize;
            for _ in 0..count {
                if offset + 4 > buf.len() {
                    break;
                }
                let record_len =
                    u32::from_ne_bytes(buf[offset..offset + 4].try_into().unwrap()) as usize;
                if record_len == 0 || offset + record_len > buf.len() {
                    break;
                }

                let record = &buf[offset..offset + record_len];
                if let Some(entry) = parse_record(record) {
                    entries.push(entry);
                }
                offset += record_len;
            }
        }

        Ok(entries)
    })
}

// ── Bundle sizing ─────────────────────────────────────────────────────────────

/// Iterative DFS to compute total physical size of a directory tree using
/// read_dir_bulk instead of fs::read_dir + stat.  Used for bundle directories
/// (.app, .framework, etc.).
fn compute_dir_size_bulk(path: &Path, seen_inodes: &SeenInodes) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match read_dir_bulk(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries {
            if entry.error != 0 {
                continue;
            }
            match entry.obj_type {
                VLNK => continue,
                VREG => {
                    // dev_id: i32 → u32 truncation avoids sign-extension before
                    // widening to u64 (matches stat's dev_t behaviour).
                    let dev = entry.dev_id as u32 as u64;
                    if seen_inodes.is_new(dev, entry.file_id, entry.link_count) {
                        total += entry.alloc_size.max(0) as u64;
                    }
                }
                VDIR => stack.push(dir.join(&entry.name)),
                _ => {}
            }
        }
    }
    total
}

// ── Inode deduplication ───────────────────────────────────────────────────────

/// Thread-safe inode tracker for hard-link deduplication.
///
/// Files with link_count == 1 are definitely not hard-linked and bypass the
/// set lookup entirely (fast path).
#[derive(Clone)]
struct SeenInodes {
    inner: Arc<std::sync::Mutex<HashSet<(u64, u64)>>>,
}

impl SeenInodes {
    fn new() -> Self {
        Self {
            inner: Arc::new(std::sync::Mutex::new(HashSet::new())),
        }
    }

    /// Returns true if this (dev, inode) pair has not been seen before.
    fn is_new(&self, dev: u64, ino: u64, link_count: u32) -> bool {
        if link_count <= 1 {
            return true;
        }
        let mut set = self.inner.lock().unwrap();
        set.insert((dev, ino))
    }
}

// ── Walk messages ──────────────────────────────────────────────────────────────

/// Messages sent from rayon worker threads to the collector thread.
enum WalkMsg {
    /// Files and subdirectory placeholders found in a directory.
    DirContents {
        /// Full path of the directory that was read.
        dir_path: PathBuf,
        /// (name, alloc_size, is_dir, mtime, crtime) for each child.
        children: Vec<(String, u64, bool, Option<i64>, Option<i64>)>,
    },
    /// A bundle directory sized as a single opaque entry.
    Bundle {
        parent_path: PathBuf,
        name: String,
        alloc_size: u64,
    },
}

// ── Bundle name detection ─────────────────────────────────────────────────────

const BUNDLE_EXTS: &[&str] = &[".app", ".framework", ".bundle", ".plugin", ".kext"];

fn is_bundle_name(name: &str) -> bool {
    BUNDLE_EXTS.iter().any(|ext| name.ends_with(ext))
}

// ── Walk config shared across threads ────────────────────────────────────────

/// Owned config that can be shared across rayon worker threads via Arc.
struct WalkConfig {
    root_dev: i32,
    #[allow(dead_code)]
    min_size: u64,
    #[allow(dead_code)]
    full: bool,
    excluded: Vec<PathBuf>,
}

// ── Per-directory worker ──────────────────────────────────────────────────────

/// Process one directory: read entries via getattrlistbulk, filter
/// symlinks and bundles, spawn sub-directory walks on rayon scope.
///
/// All shared state is accessed through Arc to satisfy the `'static` bound
/// required by `rayon::Scope::spawn`.
fn process_directory(
    path: PathBuf,
    cfg: Arc<WalkConfig>,
    seen_inodes: SeenInodes,
    tx: Sender<WalkMsg>,
    scope: &rayon::Scope<'_>,
) {
    let entries = match read_dir_bulk(&path) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut children: Vec<(String, u64, bool, Option<i64>, Option<i64>)> = Vec::new();

    for entry in entries {
        if entry.error != 0 {
            continue;
        }

        match entry.obj_type {
            VLNK => continue,

            VDIR => {
                let child_path = path.join(&entry.name);

                // getattrlistbulk returns the parent's dev_id for all children,
                // so mount points are invisible via entry.dev_id.  Stat the child
                // for the known mount-point containers.
                let child_dev = if path == Path::new("/System/Volumes")
                    || path == Path::new("/Volumes")
                {
                    use std::os::unix::fs::MetadataExt;
                    std::fs::metadata(&child_path)
                        .map(|m| m.dev() as i32)
                        .unwrap_or(entry.dev_id)
                } else {
                    entry.dev_id
                };

                // Skip cross-device mounts (network shares, USB, Time Machine, APFS sub-volumes)
                if cfg.root_dev != 0 && child_dev != cfg.root_dev {
                    continue;
                }

                // APFS firmlink exclusions — these paths duplicate content
                // already visible through firmlinks at /Users, /Applications, etc.
                if cfg.excluded.iter().any(|ex| child_path == *ex) {
                    continue;
                }

                // Bundle detection — compute total size without descending further
                if is_bundle_name(&entry.name) {
                    let size = compute_dir_size_bulk(&child_path, &seen_inodes);
                    let _ = tx.send(WalkMsg::Bundle {
                        parent_path: path.clone(),
                        name: entry.name,
                        alloc_size: size,
                    });
                    continue;
                }

                // Placeholder entry for this directory
                children.push((entry.name.clone(), 0, true, None, None));

                // Recurse on rayon thread pool
                let cfg2 = Arc::clone(&cfg);
                let seen2 = seen_inodes.clone();
                let tx2 = tx.clone();
                scope.spawn(move |s| {
                    process_directory(child_path, cfg2, seen2, tx2, s);
                });
            }

            VREG => {
                // dev_id: i32 → u32 truncation avoids sign-extension (matching stat's dev_t)
                let dev = entry.dev_id as u32 as u64;
                let alloc = if seen_inodes.is_new(dev, entry.file_id, entry.link_count) {
                    entry.alloc_size.max(0) as u64
                } else {
                    0
                };

                // Always include file for accurate directory size rollups.
                // The DB writer decides whether to write the individual file record
                // based on the size threshold.
                children.push((entry.name, alloc, false, Some(entry.mtime_sec), Some(entry.crtime_sec)));
            }

            _ => continue,
        }
    }

    if !children.is_empty() {
        let _ = tx.send(WalkMsg::DirContents {
            dir_path: path,
            children,
        });
    }
}

// ── Filesystem capability probe ───────────────────────────────────────────────

/// Check whether the volume at `path` supports reliable getattrlistbulk results.
///
/// FAT32/exFAT/NTFS via FSKit return allocsize=0 for all files, making the
/// bulk-attrs scanner report everything as 0 bytes.  Only APFS and HFS+ are
/// known to return correct allocation sizes.
pub fn supports_bulk_attrs(path: &Path) -> bool {
    use std::ffi::CString;

    let c_path = match CString::new(path.as_os_str().as_encoded_bytes()) {
        Ok(p) => p,
        Err(_) => return false,
    };

    let mut stat: MaybeUninit<libc::statfs> = MaybeUninit::uninit();
    let ret = unsafe { libc::statfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if ret != 0 {
        return false;
    }

    let stat = unsafe { stat.assume_init() };
    let fs_name = unsafe { std::ffi::CStr::from_ptr(stat.f_fstypename.as_ptr()) };
    let fs = fs_name.to_string_lossy();
    fs == "apfs" || fs == "hfs"
}

// ── Entry point ───────────────────────────────────────────────────────────────

/// Walk `root` in parallel using getattrlistbulk, writing directories and
/// qualifying files to `writer`.  Progress counters are updated atomically.
///
/// Only APFS and HFS+ volumes are supported; call `supports_bulk_attrs` before
/// invoking this function and fall back to `scan_directory` otherwise.
///
/// Design:
/// - A rayon scope dispatches per-directory work across all CPU cores.
/// - A crossbeam channel collects results on the calling thread; SQLite writes
///   happen here (single-threaded) to avoid contention.
/// - Inodes are deduplicated across the entire walk (hard-link aware).
/// - Bundles (.app, .framework, etc.) are sized as opaque single entries.
/// - Directories containing no files (directly or transitively) are omitted
///   from the database (same pruning optimization as the jwalk walker).
pub fn scan_directory_bulk(
    root: &Path,
    config: &ScanConfig,
    writer: &mut CacheWriter<'_>,
    progress: &ScanProgress,
) -> Result<HashMap<i64, (i64, i64)>> {
    // Detect root filesystem device for cross-device mount filtering
    let root_dev: i32 = {
        use std::os::unix::fs::MetadataExt;
        root.metadata().map(|m| m.dev() as i32).unwrap_or(0)
    };

    // APFS firmlink exclusions — prevent double-counting when scanning /
    let excluded = vec![
        PathBuf::from("/System/Volumes/Data"),
        PathBuf::from("/System/Volumes/Update/mnt1"),
        PathBuf::from("/System/Volumes/Update/SFR/mnt1"),
    ];

    let walk_cfg = Arc::new(WalkConfig {
        root_dev,
        min_size: config.min_file_size,
        full: config.full,
        excluded,
    });
    let seen_inodes = SeenInodes::new();

    // Generous bounded channel; back-pressure prevents unbounded memory growth
    // on extremely wide directory trees.
    let (tx, rx) = crossbeam_channel::bounded::<WalkMsg>(10_000);

    // Spawn rayon scope on a dedicated OS thread so the calling thread is free
    // to collect results and write to SQLite.
    let root_owned = root.to_path_buf();
    let tx_walk = tx.clone();
    let seen_walk = seen_inodes.clone();
    let cfg_walk = Arc::clone(&walk_cfg);
    let walk_handle = std::thread::spawn(move || {
        rayon::scope(|s| {
            process_directory(root_owned, cfg_walk, seen_walk, tx_walk, s);
        });
        // tx_walk drops here; combined with the main-thread drop below,
        // the channel closes → collector loop terminates.
    });
    // Drop the main-thread sender so the channel closes when the walk finishes.
    drop(tx);

    // ── Phase 1: collect walk results ────────────────────────────────────────
    //
    // Buffer in memory before writing to SQLite because:
    //   1. Dir IDs must be assigned before file rows can reference them.
    //   2. Empty-leaf directories should be pruned before touching the DB.
    //
    // Data structures mirror the jwalk walker for consistency.

    struct DirRecord {
        id: i64,
        parent_id: Option<i64>,
        name: String,
        parent_idx: Option<usize>, // index into dir_records for ancestor traversal
    }
    let mut dir_records: Vec<DirRecord> = Vec::new();
    let mut path_to_idx: HashMap<PathBuf, usize> = HashMap::new();
    let mut path_to_dir_id: HashMap<PathBuf, i64> = HashMap::new();
    let mut dir_id_counter: i64 = 0;

    struct PendingFile {
        dir_path: PathBuf,
        name: String,
        alloc_size: u64,
        modified_at: Option<i64>,
        created_at: Option<i64>,
    }
    let mut pending_files: Vec<PendingFile> = Vec::new();

    // Helper: register a directory (idempotent — returns existing id if already registered).
    // Because of Rust's borrow rules we can't put this in a closure that also reads the maps,
    // so it's inlined at each call site below.

    // Pre-register the root directory so files directly inside it get a known parent ID.
    {
        dir_id_counter += 1;
        let root_id = dir_id_counter;
        let root_name = root.to_string_lossy().into_owned();
        let idx = dir_records.len();
        path_to_idx.insert(root.to_path_buf(), idx);
        path_to_dir_id.insert(root.to_path_buf(), root_id);
        dir_records.push(DirRecord {
            id: root_id,
            parent_id: None,
            name: root_name,
            parent_idx: None,
        });
        progress.dirs_found.fetch_add(1, Ordering::Relaxed);
    }

    // --- Inline helper: ensure `path` is registered; return its dir_id ---
    // Implemented as a macro-style block so we can mutate all maps without
    // fighting the borrow checker.
    macro_rules! ensure_dir {
        ($path:expr, $parent_path:expr) => {{
            if let Some(&id) = path_to_dir_id.get($path) {
                id
            } else {
                dir_id_counter += 1;
                let id = dir_id_counter;
                let parent_id = $parent_path
                    .as_ref()
                    .and_then(|p: &PathBuf| path_to_dir_id.get(p))
                    .copied();
                let parent_idx = $parent_path
                    .as_ref()
                    .and_then(|p: &PathBuf| path_to_idx.get(p))
                    .copied();
                let name = ($path as &Path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| ($path as &Path).to_string_lossy().into_owned());
                let idx = dir_records.len();
                path_to_idx.insert(($path as &Path).to_path_buf(), idx);
                path_to_dir_id.insert(($path as &Path).to_path_buf(), id);
                dir_records.push(DirRecord {
                    id,
                    parent_id,
                    name,
                    parent_idx,
                });
                progress.dirs_found.fetch_add(1, Ordering::Relaxed);
                id
            }
        }};
    }

    for msg in rx {
        match msg {
            WalkMsg::DirContents { dir_path, children } => {
                let parent_path = dir_path.parent().map(|p| p.to_path_buf());
                ensure_dir!(&dir_path, parent_path);

                for (name, alloc_size, is_dir, mtime, crtime) in children {
                    if is_dir {
                        let child_path = dir_path.join(&name);
                        let dp = dir_path.clone();
                        let dp_opt = Some(dp);
                        ensure_dir!(&child_path, dp_opt);
                    } else {
                        progress.files_found.fetch_add(1, Ordering::Relaxed);
                        progress.total_size.fetch_add(alloc_size, Ordering::Relaxed);
                        pending_files.push(PendingFile {
                            dir_path: dir_path.clone(),
                            name,
                            alloc_size,
                            modified_at: mtime,
                            created_at: crtime,
                        });
                    }
                }
            }

            WalkMsg::Bundle {
                parent_path,
                name,
                alloc_size,
            } => {
                let grandparent = parent_path.parent().map(|p| p.to_path_buf());
                ensure_dir!(&parent_path, grandparent);
                progress.files_found.fetch_add(1, Ordering::Relaxed);
                progress.total_size.fetch_add(alloc_size, Ordering::Relaxed);
                pending_files.push(PendingFile {
                    dir_path: parent_path,
                    name,
                    alloc_size,
                    modified_at: None,
                    created_at: None,
                });
            }
        }
    }

    walk_handle
        .join()
        .map_err(|_| anyhow::anyhow!("Bulk walk thread panicked"))?;

    // ── Phase 2: write to SQLite ──────────────────────────────────────────────

    // Only write dirs that contain files (directly or transitively).
    let mut needed_dirs: HashSet<usize> = HashSet::new();
    let mut file_id_counter: i64 = 0;

    let write_all_files = config.full || config.min_file_size == 0;

    // Track size of small files (below threshold) per directory.
    // These won't have file records in the DB but their sizes must still
    // roll up into directory totals for accurate reporting.
    let mut skipped_size_per_dir: HashMap<i64, (i64, i64)> = HashMap::new(); // dir_id -> (count, disk_size)

    for pf in &pending_files {
        let dir_id = match path_to_dir_id.get(&pf.dir_path) {
            Some(&id) => id,
            None => continue, // orphan — parent dir was never registered
        };

        // Mark this dir and all ancestors as needed (regardless of file size).
        if let Some(&idx) = path_to_idx.get(&pf.dir_path) {
            let mut cur = Some(idx);
            while let Some(i) = cur {
                if !needed_dirs.insert(i) {
                    break; // already marked → ancestors are already done too
                }
                cur = dir_records[i].parent_idx;
            }
        }

        if write_all_files || pf.alloc_size >= config.min_file_size {
            let ext = file_extension(&pf.name);
            file_id_counter += 1;
            writer.add_file(FileEntry {
                id: file_id_counter,
                dir_id,
                name: pf.name.clone(),
                logical_size: pf.alloc_size as i64,
                disk_size: pf.alloc_size as i64,
                created_at: pf.created_at,
                modified_at: pf.modified_at,
                extension: ext,
                inode: None,
                content_hash: None,
            })?;
        } else {
            // File is below threshold — don't write a record, but track its size.
            let entry = skipped_size_per_dir.entry(dir_id).or_insert((0, 0));
            entry.0 += 1;
            entry.1 += pf.alloc_size as i64;
        }
    }

    // Write only dirs that contain files.
    for (idx, d) in dir_records.iter().enumerate() {
        if !needed_dirs.contains(&idx) {
            continue;
        }
        writer.add_dir(DirEntry {
            id: d.id,
            parent_id: d.parent_id,
            name: d.name.clone(),
            created_at: None,
            modified_at: None,
        })?;
    }

    Ok(skipped_size_per_dir)
}
