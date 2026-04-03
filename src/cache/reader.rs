use anyhow::Result;
use rusqlite::Connection;
use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

pub use crate::cache::writer::ScanMeta;

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

/// A node in the directory tree, representing either a directory or a file.
#[derive(Debug, Clone, Serialize)]
pub struct TreeNode {
    pub id: i64,
    pub name: String,
    pub is_dir: bool,
    pub disk_size: u64,
    pub logical_size: u64,
    pub file_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TreeNode>,
    /// For files: the directory ID containing this file (used to reconstruct full path).
    /// For directories: `None` (they use their own `id`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dir_id: Option<i64>,
}

/// A flat file row with a reconstructed full path.
#[derive(Debug, Clone, Serialize)]
pub struct FileRow {
    pub name: String,
    pub full_path: String,
    pub disk_size: u64,
    pub logical_size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extension: Option<String>,
}

pub fn load_scan_meta(conn: &Connection) -> Result<Option<ScanMeta>> {
    let result = conn.query_row(
        "SELECT root_path, scanned_at, total_files, total_dirs, total_size, scan_duration_ms
         FROM scan_meta LIMIT 1",
        [],
        |row| {
            Ok(ScanMeta {
                root_path: row.get(0)?,
                scanned_at: row.get(1)?,
                total_files: row.get(2)?,
                total_dirs: row.get(3)?,
                total_size: row.get(4)?,
                scan_duration_ms: row.get(5)?,
            })
        },
    );
    match result {
        Ok(meta) => Ok(Some(meta)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

// ---------------------------------------------------------------------------
// Core helpers
// ---------------------------------------------------------------------------

/// Walk the parent_id chain upward from `dir_id` and build a "/" -separated
/// path string, e.g. `"home/projects/node_modules"`.
pub fn reconstruct_path(conn: &Connection, dir_id: i64) -> Result<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = dir_id;

    loop {
        let row: (String, Option<i64>) = conn.query_row(
            "SELECT name, parent_id FROM dirs WHERE id = ?1",
            rusqlite::params![current],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        parts.push(row.0);
        match row.1 {
            Some(pid) => current = pid,
            None => break,
        }
    }

    parts.reverse();
    Ok(parts.join("/"))
}

/// Batch-reconstruct full paths for a set of `dir_id` values in a single
/// recursive CTE query, returning a map from dir_id → full path string.
pub fn reconstruct_paths(conn: &Connection, dir_ids: &[i64]) -> Result<HashMap<i64, String>> {
    if dir_ids.is_empty() {
        return Ok(HashMap::new());
    }

    // Build the ancestor chain for all requested dir_ids at once.
    // The CTE walks parent_id upward; we then group and concatenate in Rust.
    let placeholders: String = dir_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "WITH RECURSIVE ancestors(start_id, id, name, parent_id, depth) AS (
            SELECT d.id, d.id, d.name, d.parent_id, 0
            FROM dirs d WHERE d.id IN ({placeholders})
          UNION ALL
            SELECT a.start_id, d.id, d.name, d.parent_id, a.depth + 1
            FROM ancestors a JOIN dirs d ON d.id = a.parent_id
        )
        SELECT start_id, name, depth FROM ancestors ORDER BY start_id, depth DESC"
    );

    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::types::ToSql> = dir_ids
        .iter()
        .map(|id| id as &dyn rusqlite::types::ToSql)
        .collect();

    let rows = stmt.query_map(params.as_slice(), |row| {
        Ok((
            row.get::<_, i64>(0)?,  // start_id
            row.get::<_, String>(1)?, // name
        ))
    })?;

    let mut map: HashMap<i64, Vec<String>> = HashMap::new();
    for row in rows {
        let (start_id, name) = row?;
        map.entry(start_id).or_default().push(name);
    }

    Ok(map
        .into_iter()
        .map(|(id, parts)| (id, parts.join("/")))
        .collect())
}

// ---------------------------------------------------------------------------
// Tree loading
// ---------------------------------------------------------------------------

/// Return the root directory (the one whose parent_id IS NULL) as a TreeNode.
/// Returns an error if there is no root.
pub fn load_root(conn: &Connection) -> Result<TreeNode> {
    let (id, name, disk_size, logical_size, file_count, created_at, modified_at): (
        i64,
        String,
        i64,
        i64,
        i64,
        Option<i64>,
        Option<i64>,
    ) = conn.query_row(
        "SELECT id, name, total_disk_size, total_logical_size, total_file_count,
                created_at, modified_at
         FROM dirs
         WHERE parent_id IS NULL
         LIMIT 1",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
            ))
        },
    )?;

    Ok(TreeNode {
        id,
        name,
        is_dir: true,
        disk_size: disk_size as u64,
        logical_size: logical_size as u64,
        file_count: file_count as u64,
        created_at,
        modified_at,
        extension: None,
        children: Vec::new(),
        dir_id: None,
    })
}

/// Return the immediate children (sub-dirs + files) of `dir_id`, sorted by
/// disk_size descending.
pub fn load_children(conn: &Connection, dir_id: i64) -> Result<Vec<TreeNode>> {
    // Sub-directories
    let mut children: Vec<TreeNode> = {
        let mut stmt = conn.prepare(
            "SELECT id, name, total_disk_size, total_logical_size, total_file_count,
                    created_at, modified_at
             FROM dirs
             WHERE parent_id = ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![dir_id], |row| {
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
                dir_id: None,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    // Direct files
    let file_nodes: Vec<TreeNode> = {
        let mut stmt = conn.prepare(
            "SELECT id, name, disk_size, logical_size,
                    created_at, modified_at, extension
             FROM files
             WHERE dir_id = ?1",
        )?;
        let rows = stmt.query_map(rusqlite::params![dir_id], |row| {
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
                dir_id: Some(dir_id),
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    children.extend(file_nodes);
    // Sort by disk_size descending
    children.sort_by(|a, b| b.disk_size.cmp(&a.disk_size));
    Ok(children)
}

/// Recursively load a tree of directories and files up to `max_depth` levels.
/// depth=0 returns just the node with no children, depth=1 includes immediate children, etc.
pub fn load_tree_to_depth(conn: &Connection, dir_id: i64, max_depth: usize) -> Result<TreeNode> {
    // Load the directory node itself
    let (name, disk_size, logical_size, file_count, created_at, modified_at): (
        String,
        i64,
        i64,
        i64,
        Option<i64>,
        Option<i64>,
    ) = conn.query_row(
        "SELECT name, total_disk_size, total_logical_size, total_file_count, created_at, modified_at
         FROM dirs WHERE id = ?1",
        rusqlite::params![dir_id],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        },
    )?;

    let mut node = TreeNode {
        id: dir_id,
        name,
        is_dir: true,
        disk_size: disk_size as u64,
        logical_size: logical_size as u64,
        file_count: file_count as u64,
        created_at,
        modified_at,
        extension: None,
        children: Vec::new(),
        dir_id: None,
    };

    if max_depth > 0 {
        let mut children = load_children(conn, dir_id)?;
        for child in &mut children {
            if child.is_dir {
                *child = load_tree_to_depth(conn, child.id, max_depth - 1)?;
            }
        }
        node.children = children;
    }

    Ok(node)
}

// ---------------------------------------------------------------------------
// File queries
// ---------------------------------------------------------------------------

/// Raw file row before path reconstruction.
struct RawFileRow {
    dir_id: i64,
    name: String,
    disk_size: u64,
    logical_size: u64,
    created_at: Option<i64>,
    modified_at: Option<i64>,
    extension: Option<String>,
}

/// Resolve dir_id → full_path for a batch of raw rows in one recursive CTE.
fn resolve_paths(conn: &Connection, raw: Vec<RawFileRow>) -> Result<Vec<FileRow>> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    let dir_ids: Vec<i64> = raw.iter().map(|r| r.dir_id).collect();
    let path_map = reconstruct_paths(conn, &dir_ids)?;

    Ok(raw
        .into_iter()
        .map(|r| {
            let dir_path = path_map.get(&r.dir_id).cloned().unwrap_or_default();
            let full_path = format!("{}/{}", dir_path, r.name);
            FileRow {
                name: r.name,
                full_path,
                disk_size: r.disk_size,
                logical_size: r.logical_size,
                created_at: r.created_at,
                modified_at: r.modified_at,
                extension: r.extension,
            }
        })
        .collect())
}

fn query_raw_rows(
    stmt: &mut rusqlite::Statement,
    params: &[&dyn rusqlite::types::ToSql],
) -> Result<Vec<RawFileRow>> {
    let rows = stmt.query_map(params, |row| {
        Ok(RawFileRow {
            dir_id: row.get(0)?,
            name: row.get(1)?,
            disk_size: row.get::<_, i64>(2)? as u64,
            logical_size: row.get::<_, i64>(3)? as u64,
            created_at: row.get(4)?,
            modified_at: row.get(5)?,
            extension: row.get(6)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Return files whose disk_size >= `min_size`, sorted by disk_size DESC,
/// limited to `limit` rows. full_path is reconstructed.
pub fn query_large_files(
    conn: &Connection,
    min_size: u64,
    limit: usize,
) -> Result<Vec<FileRow>> {
    let mut stmt = conn.prepare(
        "SELECT f.dir_id, f.name, f.disk_size, f.logical_size,
                f.created_at, f.modified_at, f.extension
         FROM files f
         WHERE f.disk_size >= ?1
         ORDER BY f.disk_size DESC
         LIMIT ?2",
    )?;
    let min = min_size as i64;
    let lim = limit as i64;
    let raw = query_raw_rows(&mut stmt, &[&min, &lim])?;
    resolve_paths(conn, raw)
}

/// Return files modified after `after_timestamp`, sorted by modified_at DESC,
/// limited to `limit` rows.
pub fn query_recent_files(
    conn: &Connection,
    after_timestamp: i64,
    limit: usize,
) -> Result<Vec<FileRow>> {
    let mut stmt = conn.prepare(
        "SELECT f.dir_id, f.name, f.disk_size, f.logical_size,
                f.created_at, f.modified_at, f.extension
         FROM files f
         WHERE f.modified_at > ?1
         ORDER BY f.modified_at DESC
         LIMIT ?2",
    )?;
    let lim = limit as i64;
    let raw = query_raw_rows(&mut stmt, &[&after_timestamp, &lim])?;
    resolve_paths(conn, raw)
}

/// Return files with the given extension, sorted by disk_size DESC,
/// limited to `limit` rows. full_path is reconstructed.
pub fn query_by_extension(
    conn: &Connection,
    ext: &str,
    limit: usize,
) -> Result<Vec<FileRow>> {
    let mut stmt = conn.prepare(
        "SELECT f.dir_id, f.name, f.disk_size, f.logical_size,
                f.created_at, f.modified_at, f.extension
         FROM files f
         WHERE f.extension = ?1
         ORDER BY f.disk_size DESC
         LIMIT ?2",
    )?;
    let lim = limit as i64;
    let raw = query_raw_rows(&mut stmt, &[&ext, &lim])?;
    resolve_paths(conn, raw)
}

/// Return files whose name contains `pattern` (case-insensitive), sorted by
/// disk_size DESC, limited to `limit` rows. full_path is reconstructed.
pub fn query_by_name(
    conn: &Connection,
    pattern: &str,
    limit: usize,
) -> Result<Vec<FileRow>> {
    let mut stmt = conn.prepare(
        "SELECT f.dir_id, f.name, f.disk_size, f.logical_size,
                f.created_at, f.modified_at, f.extension
         FROM files f
         WHERE LOWER(f.name) LIKE '%' || LOWER(?1) || '%'
         ORDER BY f.disk_size DESC
         LIMIT ?2",
    )?;
    let lim = limit as i64;
    let raw = query_raw_rows(&mut stmt, &[&pattern, &lim])?;
    resolve_paths(conn, raw)
}

/// Summary data combining multiple analyses for a one-shot cleanup report.
#[derive(Debug, Clone, Serialize)]
pub struct CleanupSummary {
    pub total_size: u64,
    pub total_files: u64,
    pub large_files: Vec<FileRow>,
    pub dev_artifacts: Vec<TreeNode>,
    pub old_files_size: u64,
    pub old_files_count: u64,
    pub potential_savings: u64,
}

/// Build a cleanup summary by combining large files, dev artifacts, and old
/// file statistics.
pub fn query_summary(conn: &Connection) -> Result<CleanupSummary> {
    // Total size and file count from root dir aggregate
    let (total_size, total_files): (u64, u64) = conn
        .query_row(
            "SELECT COALESCE(total_disk_size, 0), COALESCE(total_file_count, 0)
             FROM dirs WHERE parent_id IS NULL",
            [],
            |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64)),
        )
        .unwrap_or((0, 0));

    // Top 10 largest files (>= 100 MB)
    let large_files = query_large_files(conn, 100 * 1024 * 1024, 10)?;

    // Dev artifact directories
    let dev_artifacts = query_dev_artifacts(conn)?;

    // Old files: files not modified in the past 365 days
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let cutoff = now - (365 * 86400);

    let (old_files_size, old_files_count): (u64, u64) = conn
        .query_row(
            "SELECT COALESCE(SUM(disk_size), 0), COUNT(*)
             FROM files WHERE modified_at < ?1",
            rusqlite::params![cutoff],
            |row| Ok((row.get::<_, i64>(0)? as u64, row.get::<_, i64>(1)? as u64)),
        )
        .unwrap_or((0, 0));

    let dev_artifacts_size: u64 = dev_artifacts.iter().map(|n| n.disk_size).sum();
    let potential_savings = dev_artifacts_size + old_files_size;

    Ok(CleanupSummary {
        total_size,
        total_files,
        large_files,
        dev_artifacts,
        old_files_size,
        old_files_count,
        potential_savings,
    })
}

/// Return files modified before `before_timestamp`, sorted by disk_size DESC,
/// limited to `limit` rows.
pub fn query_old_files(
    conn: &Connection,
    before_timestamp: i64,
    limit: usize,
) -> Result<Vec<FileRow>> {
    let mut stmt = conn.prepare(
        "SELECT f.dir_id, f.name, f.disk_size, f.logical_size,
                f.created_at, f.modified_at, f.extension
         FROM files f
         WHERE f.modified_at < ?1
         ORDER BY f.disk_size DESC
         LIMIT ?2",
    )?;
    let lim = limit as i64;
    let raw = query_raw_rows(&mut stmt, &[&before_timestamp, &lim])?;
    resolve_paths(conn, raw)
}

/// Return directories whose name matches a well-known dev artifact pattern.
pub fn query_dev_artifacts(conn: &Connection) -> Result<Vec<TreeNode>> {
    const ARTIFACT_NAMES: &[&str] = &[
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

    let placeholders: String = ARTIFACT_NAMES
        .iter()
        .enumerate()
        .map(|(i, _)| format!("?{}", i + 1))
        .collect::<Vec<_>>()
        .join(", ");

    let sql = format!(
        "SELECT id, name, total_disk_size, total_logical_size, total_file_count,
                created_at, modified_at
         FROM dirs
         WHERE name IN ({})
         ORDER BY total_disk_size DESC",
        placeholders
    );

    let mut stmt = conn.prepare(&sql)?;

    // Build the params dynamically using rusqlite's raw parameter binding
    let params: Vec<&dyn rusqlite::types::ToSql> = ARTIFACT_NAMES
        .iter()
        .map(|s| s as &dyn rusqlite::types::ToSql)
        .collect();

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
            dir_id: None,
        })
    })?;

    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Search for directories by name (case-insensitive substring match).
pub fn query_dirs_by_name(conn: &Connection, name: &str, limit: usize) -> Result<Vec<TreeNode>> {
    let pattern = format!("%{}%", name);
    let mut stmt = conn.prepare(
        "SELECT id, name, total_disk_size, total_logical_size, total_file_count,
                created_at, modified_at
         FROM dirs
         WHERE name LIKE ?1
         ORDER BY total_disk_size DESC
         LIMIT ?2",
    )?;
    let rows = stmt.query_map(rusqlite::params![pattern, limit as i64], |row| {
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
            dir_id: None,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

// ---------------------------------------------------------------------------
// Duplicate detection
// ---------------------------------------------------------------------------

/// A group of files that share identical content (same blake3 hash).
#[derive(Debug, Clone, Serialize)]
pub struct DuplicateGroup {
    pub hash: String,
    pub size: u64,
    pub count: usize,
    pub file_ids: Vec<i64>,
}

/// Hash a file at `path` using blake3, reading in 64 KB chunks.
/// Returns the hex-encoded hash string.
fn hash_file(path: &Path) -> Result<String> {
    const BUF_SIZE: usize = 64 * 1024;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; BUF_SIZE];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Find duplicate files by content.
///
/// 1. Queries the DB for files where at least two entries share the same
///    `disk_size` (size-based candidates).
/// 2. For each candidate, reconstructs its full path, hashes the real file,
///    and updates `content_hash` in the DB.
/// 3. Calls `on_progress(hashed_count, total_candidates)` after each file.
/// 4. Groups candidates by `content_hash` where count > 1, sorted by size
///    descending.
pub fn find_duplicates(
    conn: &Connection,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<Vec<DuplicateGroup>> {
    // Step 1: find candidate file ids (files sharing disk_size with >= 1 other)
    let candidates: Vec<(i64, i64, i64)> = {
        let mut stmt = conn.prepare(
            "SELECT f.id, f.dir_id, f.disk_size
             FROM files f
             WHERE f.disk_size IN (
                 SELECT disk_size FROM files
                 GROUP BY disk_size
                 HAVING COUNT(*) > 1
             )
             ORDER BY f.disk_size DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };

    let total = candidates.len();

    // Step 2: batch-resolve all dir_ids → paths in one recursive CTE query.
    let dir_ids: Vec<i64> = candidates.iter().map(|(_, dir_id, _)| *dir_id).collect();
    let dir_path_map = reconstruct_paths(conn, &dir_ids)?;

    // Step 3: hash each candidate and update the DB
    for (idx, (file_id, dir_id, _disk_size)) in candidates.iter().enumerate() {
        // Reconstruct the name for this file
        let name: String = conn.query_row(
            "SELECT name FROM files WHERE id = ?1",
            rusqlite::params![file_id],
            |row| row.get(0),
        )?;
        let dir_path = dir_path_map.get(dir_id).cloned().unwrap_or_default();
        let full_path = format!("{}/{}", dir_path, name);

        // Hash the actual file (ignore if it can't be read)
        if let Ok(hash) = hash_file(Path::new(&full_path)) {
            conn.execute(
                "UPDATE files SET content_hash = ?1 WHERE id = ?2",
                rusqlite::params![hash, file_id],
            )?;
        }

        on_progress(idx + 1, total);
    }

    // Step 3: group by content_hash where count > 1
    let rows: Vec<(String, i64, i64)> = {
        let mut stmt = conn.prepare(
            "SELECT content_hash, disk_size, id
             FROM files
             WHERE content_hash IS NOT NULL
             ORDER BY disk_size DESC, content_hash",
        )?;
        let iter = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        iter.collect::<rusqlite::Result<Vec<_>>>()?
    };

    // Aggregate into groups, preserving size-desc order from SQL
    let mut map: HashMap<String, (u64, Vec<i64>)> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for (hash, size, id) in rows {
        let entry = map.entry(hash.clone()).or_insert_with(|| {
            order.push(hash.clone());
            (size as u64, Vec::<i64>::new())
        });
        entry.1.push(id);
    }

    let mut result: Vec<DuplicateGroup> = map
        .into_iter()
        .filter(|(_, (_, ids))| ids.len() > 1)
        .map(|(hash, (size, file_ids))| DuplicateGroup {
            count: file_ids.len(),
            hash,
            size,
            file_ids,
        })
        .collect();

    // Sort by size descending
    result.sort_by(|a, b| b.size.cmp(&a.size));

    Ok(result)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::schema::{create_tables, open_memory_db};
    use crate::cache::writer::{CacheWriter, DirEntry, FileEntry};
    use std::io::Write;

    /// Build the shared test DB:
    ///   home/ (id=1)  ← root
    ///     projects/ (id=2)
    ///       node_modules/ (id=3)
    ///         react.js  (2 MB, modified_at=1000)
    ///       old.log    (100 KB, modified_at=100)
    ///     big.zip      (500 MB, modified_at=9000)
    fn setup_db() -> Connection {
        let conn = open_memory_db().unwrap();
        create_tables(&conn).unwrap();

        // We need a mutable conn for the writer; use a local mut binding.
        let mut conn = conn;

        {
            let mut w = CacheWriter::new(&mut conn, 100);

            w.add_dir(DirEntry {
                id: 1,
                parent_id: None,
                name: "home".into(),
                created_at: None,
                modified_at: None,
            })
            .unwrap();
            w.add_dir(DirEntry {
                id: 2,
                parent_id: Some(1),
                name: "projects".into(),
                created_at: None,
                modified_at: None,
            })
            .unwrap();
            w.add_dir(DirEntry {
                id: 3,
                parent_id: Some(2),
                name: "node_modules".into(),
                created_at: None,
                modified_at: None,
            })
            .unwrap();

            const MB: i64 = 1024 * 1024;
            const KB: i64 = 1024;

            w.add_file(FileEntry {
                id: 1,
                dir_id: 1,
                name: "big.zip".into(),
                logical_size: 500 * MB,
                disk_size: 500 * MB,
                created_at: None,
                modified_at: Some(9000),
                extension: Some("zip".into()),
                inode: None,
                content_hash: None,
            })
            .unwrap();
            w.add_file(FileEntry {
                id: 2,
                dir_id: 2,
                name: "old.log".into(),
                logical_size: 100 * KB,
                disk_size: 100 * KB,
                created_at: None,
                modified_at: Some(100),
                extension: Some("log".into()),
                inode: None,
                content_hash: None,
            })
            .unwrap();
            w.add_file(FileEntry {
                id: 3,
                dir_id: 3,
                name: "react.js".into(),
                logical_size: 2 * MB,
                disk_size: 2 * MB,
                created_at: None,
                modified_at: Some(1000),
                extension: Some("js".into()),
                inode: None,
                content_hash: None,
            })
            .unwrap();

            w.finalize().unwrap();
        }

        conn
    }

    #[test]
    fn test_load_root_returns_home() -> Result<()> {
        let conn = setup_db();
        let root = load_root(&conn)?;
        assert_eq!(root.name, "home");
        assert!(root.disk_size > 0, "root disk_size should be > 0 after rollup");
        Ok(())
    }

    #[test]
    fn test_load_children_of_root() -> Result<()> {
        let conn = setup_db();
        let children = load_children(&conn, 1)?;

        // Should have exactly 2 children: "projects" dir + "big.zip" file
        assert_eq!(children.len(), 2);

        let names: Vec<&str> = children.iter().map(|n| n.name.as_str()).collect();
        assert!(names.contains(&"projects"), "expected 'projects' in children");
        assert!(names.contains(&"big.zip"), "expected 'big.zip' in children");

        // big.zip (500 MB) is larger than projects, so it should come first
        assert_eq!(children[0].name, "big.zip");
        Ok(())
    }

    #[test]
    fn test_query_large_files_above_1mb() -> Result<()> {
        let conn = setup_db();
        const MB: u64 = 1024 * 1024;

        let files = query_large_files(&conn, MB, 100)?;

        // big.zip (500 MB) and react.js (2 MB) are above 1 MB; old.log (100 KB) is not
        assert_eq!(files.len(), 2);

        // Sorted by size desc: big.zip first
        assert_eq!(files[0].name, "big.zip");
        assert_eq!(files[1].name, "react.js");

        Ok(())
    }

    #[test]
    fn test_query_recent_files_after_5000() -> Result<()> {
        let conn = setup_db();

        let files = query_recent_files(&conn, 5000, 100)?;

        // Only big.zip has modified_at=9000 > 5000
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "big.zip");
        Ok(())
    }

    #[test]
    fn test_query_old_files_before_500() -> Result<()> {
        let conn = setup_db();

        let files = query_old_files(&conn, 500, 100)?;

        // Only old.log has modified_at=100 < 500
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].name, "old.log");
        Ok(())
    }

    #[test]
    fn test_query_dev_artifacts() -> Result<()> {
        let conn = setup_db();

        let artifacts = query_dev_artifacts(&conn)?;

        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].name, "node_modules");
        Ok(())
    }

    #[test]
    fn test_reconstruct_path() -> Result<()> {
        let conn = setup_db();

        let path = reconstruct_path(&conn, 3)?;
        assert_eq!(path, "home/projects/node_modules");
        Ok(())
    }

    /// Sets up a DB whose file paths point to real temp files so that
    /// find_duplicates can hash them.
    #[test]
    fn test_find_duplicates_detects_identical_files() -> Result<()> {
        use tempfile::TempDir;

        let tmp = TempDir::new()?;
        let tmp_path = tmp.path().to_string_lossy().into_owned();

        // Write three files: two with identical content, one unique.
        let dup_content = b"hello duplicate world";
        let unique_content = b"this is unique content 12345";

        let file_a = tmp.path().join("file_a.txt");
        let file_b = tmp.path().join("file_b.txt");
        let file_c = tmp.path().join("file_c.txt");
        std::fs::File::create(&file_a)?.write_all(dup_content)?;
        std::fs::File::create(&file_b)?.write_all(dup_content)?;
        std::fs::File::create(&file_c)?.write_all(unique_content)?;

        let dup_size = dup_content.len() as i64;
        let unique_size = unique_content.len() as i64;

        // Build an in-memory DB where dir name == tmp_path so that
        // reconstruct_path(dir_id) + "/" + name gives the correct absolute path.
        let conn = open_memory_db()?;
        create_tables(&conn)?;
        let mut conn = conn;

        {
            let mut w = CacheWriter::new(&mut conn, 100);

            // Root dir whose name IS the tmp path (no parent → reconstruct
            // returns just that single segment, giving "<tmp_path>/file_x.txt")
            w.add_dir(DirEntry {
                id: 1,
                parent_id: None,
                name: tmp_path.clone(),
                created_at: None,
                modified_at: None,
            })?;

            w.add_file(FileEntry {
                id: 1,
                dir_id: 1,
                name: "file_a.txt".into(),
                logical_size: dup_size,
                disk_size: dup_size,
                created_at: None,
                modified_at: None,
                extension: Some("txt".into()),
                inode: None,
                content_hash: None,
            })?;
            w.add_file(FileEntry {
                id: 2,
                dir_id: 1,
                name: "file_b.txt".into(),
                logical_size: dup_size,
                disk_size: dup_size,
                created_at: None,
                modified_at: None,
                extension: Some("txt".into()),
                inode: None,
                content_hash: None,
            })?;
            w.add_file(FileEntry {
                id: 3,
                dir_id: 1,
                name: "file_c.txt".into(),
                logical_size: unique_size,
                disk_size: unique_size,
                created_at: None,
                modified_at: None,
                extension: Some("txt".into()),
                inode: None,
                content_hash: None,
            })?;

            w.finalize()?;
        }

        let mut progress_calls = Vec::new();
        let groups = find_duplicates(&conn, |done, total| {
            progress_calls.push((done, total));
        })?;

        // file_c is unique (no size-sibling), so only file_a + file_b are candidates
        assert_eq!(progress_calls.len(), 2, "expected 2 progress calls");
        assert_eq!(progress_calls.last(), Some(&(2, 2)));

        // Exactly one duplicate group
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.count, 2);
        assert_eq!(g.size, dup_size as u64);
        assert!(g.file_ids.contains(&1));
        assert!(g.file_ids.contains(&2));

        Ok(())
    }

    #[test]
    fn test_reconstruct_path_root_node() -> Result<()> {
        let conn = setup_db();
        // Root node (id=1) has no parent — should return just "home"
        let path = reconstruct_path(&conn, 1)?;
        assert_eq!(path, "home");
        Ok(())
    }

    #[test]
    fn test_load_children_leaf_directory() -> Result<()> {
        // Create a dedicated DB with a leaf dir that has no children at all.
        let conn2 = open_memory_db()?;
        create_tables(&conn2)?;
        let mut conn2 = conn2;
        {
            let mut w = CacheWriter::new(&mut conn2, 100);
            w.add_dir(DirEntry {
                id: 1,
                parent_id: None,
                name: "root".into(),
                created_at: None,
                modified_at: None,
            })?;
            w.add_dir(DirEntry {
                id: 2,
                parent_id: Some(1),
                name: "empty_leaf".into(),
                created_at: None,
                modified_at: None,
            })?;
            w.finalize()?;
        }

        let children = load_children(&conn2, 2)?;
        assert!(children.is_empty(), "leaf directory with no children should return empty Vec");
        Ok(())
    }

    #[test]
    fn test_load_scan_meta_empty() -> Result<()> {
        let conn = open_memory_db()?;
        create_tables(&conn)?;
        let meta = load_scan_meta(&conn)?;
        assert!(meta.is_none());
        Ok(())
    }

    #[test]
    fn test_load_scan_meta_with_data() -> Result<()> {
        let conn = setup_db();
        // Insert a scan_meta row
        conn.execute(
            "INSERT INTO scan_meta (root_path, scanned_at, total_files, total_dirs, total_size, scan_duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params!["/home", 1000i64, 3i64, 3i64, 502i64 * 1024 * 1024, 500i64],
        )?;
        let meta = load_scan_meta(&conn)?.expect("should have scan meta");
        assert_eq!(meta.root_path, "/home");
        assert_eq!(meta.total_files, 3);
        Ok(())
    }

    #[test]
    fn test_load_tree_depth_zero() -> Result<()> {
        let conn = setup_db();
        let tree = load_tree_to_depth(&conn, 1, 0)?;
        assert_eq!(tree.name, "home");
        assert!(tree.children.is_empty(), "depth=0 should have no children");
        Ok(())
    }

    #[test]
    fn test_load_tree_depth_one() -> Result<()> {
        let conn = setup_db();
        let tree = load_tree_to_depth(&conn, 1, 1)?;
        assert_eq!(tree.name, "home");
        assert_eq!(tree.children.len(), 2, "should have projects dir + big.zip");
        // Children should not have their own children loaded
        for child in &tree.children {
            assert!(child.children.is_empty(), "depth=1 children should have no grandchildren");
        }
        Ok(())
    }

    #[test]
    fn test_load_tree_depth_two() -> Result<()> {
        let conn = setup_db();
        let tree = load_tree_to_depth(&conn, 1, 2)?;
        let projects = tree
            .children
            .iter()
            .find(|c| c.name == "projects")
            .expect("should find projects");
        assert!(!projects.children.is_empty(), "depth=2 should load projects' children");
        Ok(())
    }

    #[test]
    fn test_serialize_tree_node_json() -> Result<()> {
        let conn = setup_db();
        let tree = load_tree_to_depth(&conn, 1, 1)?;
        let json = serde_json::to_string_pretty(&tree)?;
        assert!(json.contains("\"name\""));
        assert!(json.contains("\"disk_size\""));
        // Empty children should be omitted due to skip_serializing_if
        // (big.zip has no children and is not a dir, so children field is empty)
        Ok(())
    }
}
