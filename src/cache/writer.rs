use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashMap;

// ---------------------------------------------------------------------------
// Entry types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DirEntry {
    /// Stable ID assigned by the caller (tree-walk order).
    pub id: i64,
    /// `None` for the root directory.
    pub parent_id: Option<i64>,
    pub name: String,
    pub created_at: Option<i64>,
    pub modified_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub id: i64,
    pub dir_id: i64,
    pub name: String,
    pub logical_size: i64,
    pub disk_size: i64,
    pub created_at: Option<i64>,
    pub modified_at: Option<i64>,
    pub extension: Option<String>,
    pub inode: Option<i64>,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ScanMeta {
    pub root_path: String,
    pub scanned_at: i64,
    pub total_files: i64,
    pub total_dirs: i64,
    pub total_size: i64,
    pub scan_duration_ms: i64,
}

// ---------------------------------------------------------------------------
// CacheWriter
// ---------------------------------------------------------------------------

pub struct CacheWriter<'conn> {
    conn: &'conn mut Connection,
    batch_size: usize,
    pub dir_buf: Vec<DirEntry>,
    pub file_buf: Vec<FileEntry>,
}

impl<'conn> CacheWriter<'conn> {
    /// Creates a new writer. `batch_size` controls how many entries are buffered
    /// before a flush is triggered automatically.
    pub fn new(conn: &'conn mut Connection, batch_size: usize) -> Self {
        let batch_size = batch_size.max(1);
        Self {
            conn,
            batch_size,
            dir_buf: Vec::with_capacity(batch_size),
            file_buf: Vec::with_capacity(batch_size),
        }
    }

    /// Buffer a directory entry, flushing if the buffer is full.
    pub fn add_dir(&mut self, entry: DirEntry) -> Result<()> {
        self.dir_buf.push(entry);
        if self.dir_buf.len() >= self.batch_size {
            self.flush_dirs()?;
        }
        Ok(())
    }

    /// Buffer a file entry, flushing if the buffer is full.
    pub fn add_file(&mut self, entry: FileEntry) -> Result<()> {
        self.file_buf.push(entry);
        if self.file_buf.len() >= self.batch_size {
            self.flush_files()?;
        }
        Ok(())
    }

    /// Insert all buffered directory entries in a single transaction.
    pub fn flush_dirs(&mut self) -> Result<()> {
        if self.dir_buf.is_empty() {
            return Ok(());
        }

        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO dirs (id, parent_id, name, created_at, modified_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )?;
            for d in &self.dir_buf {
                stmt.execute(rusqlite::params![
                    d.id,
                    d.parent_id,
                    d.name,
                    d.created_at,
                    d.modified_at,
                ])?;
            }
        }
        tx.commit()?;
        self.dir_buf.clear();
        Ok(())
    }

    /// Insert all buffered file entries in a single transaction.
    pub fn flush_files(&mut self) -> Result<()> {
        if self.file_buf.is_empty() {
            return Ok(());
        }

        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO files
                    (id, dir_id, name, logical_size, disk_size,
                     created_at, modified_at, extension, inode, content_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            )?;
            for f in &self.file_buf {
                stmt.execute(rusqlite::params![
                    f.id,
                    f.dir_id,
                    f.name,
                    f.logical_size,
                    f.disk_size,
                    f.created_at,
                    f.modified_at,
                    f.extension,
                    f.inode,
                    f.content_hash,
                ])?;
            }
        }
        tx.commit()?;
        self.file_buf.clear();
        Ok(())
    }

    /// Flush remaining buffers, then compute aggregated dir sizes bottom-up.
    pub fn finalize(&mut self) -> Result<()> {
        self.flush_dirs()?;
        self.flush_files()?;
        self.compute_dir_sizes()?;
        Ok(())
    }

    /// Bottom-up aggregation using a Rust-side topological sort:
    /// 1. Set each dir's direct file_count/sizes from its own files.
    /// 2. Load dir topology, sort leaves-first, then roll up into parents.
    pub fn compute_dir_sizes(&mut self) -> Result<()> {
        // Step 1: direct file stats per dir.
        self.conn.execute_batch(
            "UPDATE dirs
             SET file_count         = (SELECT COUNT(*)
                                       FROM files WHERE files.dir_id = dirs.id),
                 total_file_count   = (SELECT COUNT(*)
                                       FROM files WHERE files.dir_id = dirs.id),
                 total_logical_size = (SELECT COALESCE(SUM(logical_size), 0)
                                       FROM files WHERE files.dir_id = dirs.id),
                 total_disk_size    = (SELECT COALESCE(SUM(disk_size), 0)
                                       FROM files WHERE files.dir_id = dirs.id);",
        )?;

        // Step 2: load dir topology.
        struct DirRow {
            id: i64,
            parent_id: Option<i64>,
            tfc: i64,
            tls: i64,
            tds: i64,
        }

        let rows: Vec<DirRow> = {
            let mut stmt = self.conn.prepare(
                "SELECT id, parent_id, total_file_count, total_logical_size, total_disk_size
                 FROM dirs",
            )?;
            let result = stmt.query_map([], |row| {
                Ok(DirRow {
                    id: row.get(0)?,
                    parent_id: row.get(1)?,
                    tfc: row.get(2)?,
                    tls: row.get(3)?,
                    tds: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
            result
        };

        let n = rows.len();

        // Build index: id -> position in vec.
        let mut idx: HashMap<i64, usize> = HashMap::with_capacity(n);
        let mut tfc: Vec<i64> = Vec::with_capacity(n);
        let mut tls: Vec<i64> = Vec::with_capacity(n);
        let mut tds: Vec<i64> = Vec::with_capacity(n);
        let mut parents: Vec<Option<i64>> = Vec::with_capacity(n);
        let mut ids: Vec<i64> = Vec::with_capacity(n);

        for (i, r) in rows.iter().enumerate() {
            idx.insert(r.id, i);
            tfc.push(r.tfc);
            tls.push(r.tls);
            tds.push(r.tds);
            parents.push(r.parent_id);
            ids.push(r.id);
        }

        // Compute subtree height of each node.
        // After convergence, leaves have height=0, root has the largest value.
        // We iterate n times to handle any ordering in the query result.
        let mut height = vec![0usize; n];
        for _ in 0..n {
            for i in 0..n {
                if let Some(pid) = parents[i] {
                    if let Some(&pi) = idx.get(&pid) {
                        // Parent height must exceed child height.
                        if height[pi] <= height[i] {
                            height[pi] = height[i] + 1;
                        }
                    }
                }
            }
        }

        // Sort by height ascending → leaves (height=0) come first, root last.
        // This guarantees children are rolled up before their parent is processed.
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&i| height[i]);

        // Step 3: roll up leaf → parent (children processed before parents).
        for &i in &order {
            let (add_tfc, add_tls, add_tds) = (tfc[i], tls[i], tds[i]);
            if let Some(pid) = parents[i] {
                if let Some(&pi) = idx.get(&pid) {
                    tfc[pi] += add_tfc;
                    tls[pi] += add_tls;
                    tds[pi] += add_tds;
                }
            }
        }

        // Step 4: persist updated totals.
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "UPDATE dirs
                 SET total_file_count   = ?2,
                     total_logical_size = ?3,
                     total_disk_size    = ?4
                 WHERE id = ?1",
            )?;
            for i in 0..n {
                stmt.execute(rusqlite::params![ids[i], tfc[i], tls[i], tds[i]])?;
            }
        }
        tx.commit()?;

        Ok(())
    }

    /// Insert scan metadata (call after finalize).
    pub fn write_meta(&mut self, meta: &ScanMeta) -> Result<()> {
        self.conn.execute(
            "INSERT INTO scan_meta
                (root_path, scanned_at, total_files, total_dirs, total_size, scan_duration_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                meta.root_path,
                meta.scanned_at,
                meta.total_files,
                meta.total_dirs,
                meta.total_size,
                meta.scan_duration_ms,
            ],
        )?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::schema::{create_tables, open_memory_db};

    fn make_conn() -> Connection {
        let conn = open_memory_db().unwrap();
        create_tables(&conn).unwrap();
        conn
    }

    /// Build a two-level tree:
    ///   root/ (id=1)
    ///     file_a.txt  (100 logical, 4096 disk)
    ///     child/ (id=2)
    ///       file_b.txt (200 logical, 4096 disk)
    ///       file_c.bin (300 logical, 8192 disk)
    ///
    /// After finalize():
    ///   child: file_count=2, total_file_count=2, total_logical=500, total_disk=12288
    ///   root:  file_count=1, total_file_count=3, total_logical=600, total_disk=16384
    #[test]
    fn test_finalize_rolls_up_sizes() -> Result<()> {
        let mut conn = make_conn();
        let mut w = CacheWriter::new(&mut conn, 100);

        w.add_dir(DirEntry {
            id: 1, parent_id: None, name: "root".into(),
            created_at: None, modified_at: None,
        })?;
        w.add_dir(DirEntry {
            id: 2, parent_id: Some(1), name: "child".into(),
            created_at: None, modified_at: None,
        })?;

        w.add_file(FileEntry {
            id: 1, dir_id: 1, name: "file_a.txt".into(),
            logical_size: 100, disk_size: 4096,
            created_at: None, modified_at: None,
            extension: Some("txt".into()), inode: None, content_hash: None,
        })?;
        w.add_file(FileEntry {
            id: 2, dir_id: 2, name: "file_b.txt".into(),
            logical_size: 200, disk_size: 4096,
            created_at: None, modified_at: None,
            extension: Some("txt".into()), inode: None, content_hash: None,
        })?;
        w.add_file(FileEntry {
            id: 3, dir_id: 2, name: "file_c.bin".into(),
            logical_size: 300, disk_size: 8192,
            created_at: None, modified_at: None,
            extension: Some("bin".into()), inode: None, content_hash: None,
        })?;

        w.finalize()?;

        // Verify child dir
        let (child_fc, child_tfc, child_ls, child_ds): (i64, i64, i64, i64) =
            conn.query_row(
                "SELECT file_count, total_file_count, total_logical_size, total_disk_size
                 FROM dirs WHERE id = 2",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        assert_eq!(child_fc,  2,     "child file_count");
        assert_eq!(child_tfc, 2,     "child total_file_count");
        assert_eq!(child_ls,  500,   "child total_logical_size");
        assert_eq!(child_ds,  12288, "child total_disk_size");

        // Verify root dir
        let (root_fc, root_tfc, root_ls, root_ds): (i64, i64, i64, i64) =
            conn.query_row(
                "SELECT file_count, total_file_count, total_logical_size, total_disk_size
                 FROM dirs WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        assert_eq!(root_fc,  1,     "root file_count");
        assert_eq!(root_tfc, 3,     "root total_file_count (1 direct + 2 from child)");
        assert_eq!(root_ls,  600,   "root total_logical_size");
        assert_eq!(root_ds,  16384, "root total_disk_size");

        Ok(())
    }

    #[test]
    fn test_batch_flush_triggers_at_batch_size() -> Result<()> {
        let mut conn = make_conn();
        let mut w = CacheWriter::new(&mut conn, 2); // batch_size=2

        // Add root dir first (needed for FK on files).
        w.add_dir(DirEntry {
            id: 1, parent_id: None, name: "root".into(),
            created_at: None, modified_at: None,
        })?;
        w.flush_dirs()?;

        // After adding 2 files the buffer should auto-flush.
        w.add_file(FileEntry {
            id: 1, dir_id: 1, name: "a".into(), logical_size: 1, disk_size: 1,
            created_at: None, modified_at: None, extension: None, inode: None, content_hash: None,
        })?;
        w.add_file(FileEntry {
            id: 2, dir_id: 1, name: "b".into(), logical_size: 1, disk_size: 1,
            created_at: None, modified_at: None, extension: None, inode: None, content_hash: None,
        })?;
        // Buffer should be empty now (auto-flushed at batch_size=2).
        assert!(w.file_buf.is_empty(), "buffer should have been auto-flushed");

        // Third file stays in buffer until explicit flush.
        w.add_file(FileEntry {
            id: 3, dir_id: 1, name: "c".into(), logical_size: 1, disk_size: 1,
            created_at: None, modified_at: None, extension: None, inode: None, content_hash: None,
        })?;
        assert_eq!(w.file_buf.len(), 1);

        w.flush_files()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
        assert_eq!(count, 3);
        Ok(())
    }

    #[test]
    fn test_write_meta() -> Result<()> {
        let mut conn = make_conn();
        let mut w = CacheWriter::new(&mut conn, 100);

        let meta = ScanMeta {
            root_path: "/home/user".into(),
            scanned_at: 1_700_000_000,
            total_files: 42,
            total_dirs: 7,
            total_size: 1024 * 1024,
            scan_duration_ms: 350,
        };
        w.write_meta(&meta)?;

        let (root_path, total_files): (String, i64) = conn.query_row(
            "SELECT root_path, total_files FROM scan_meta WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(root_path, "/home/user");
        assert_eq!(total_files, 42);
        Ok(())
    }

    #[test]
    fn test_deep_three_level_rollup() -> Result<()> {
        let mut conn = make_conn();
        let mut w = CacheWriter::new(&mut conn, 100);

        // root/ -> mid/ -> leaf/
        // leaf has 1 file: 1000 disk
        w.add_dir(DirEntry { id: 1, parent_id: None,    name: "root".into(), created_at: None, modified_at: None })?;
        w.add_dir(DirEntry { id: 2, parent_id: Some(1), name: "mid".into(),  created_at: None, modified_at: None })?;
        w.add_dir(DirEntry { id: 3, parent_id: Some(2), name: "leaf".into(), created_at: None, modified_at: None })?;

        w.add_file(FileEntry {
            id: 1, dir_id: 3, name: "deep.bin".into(),
            logical_size: 500, disk_size: 1000,
            created_at: None, modified_at: None,
            extension: Some("bin".into()), inode: None, content_hash: None,
        })?;

        w.finalize()?;

        let root_ds: i64 = conn.query_row(
            "SELECT total_disk_size FROM dirs WHERE id = 1",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(root_ds, 1000, "root must roll up through mid -> leaf");

        let mid_ds: i64 = conn.query_row(
            "SELECT total_disk_size FROM dirs WHERE id = 2",
            [],
            |r| r.get(0),
        )?;
        assert_eq!(mid_ds, 1000, "mid must roll up from leaf");
        Ok(())
    }
}
