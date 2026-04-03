use anyhow::Result;
use rusqlite::Connection;
use std::path::Path;

/// Opens a SQLite database at `path` with performance PRAGMAs set.
pub fn open_db(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    apply_pragmas(&conn)?;
    Ok(conn)
}

/// Opens an in-memory SQLite database (for tests).
pub fn open_memory_db() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    apply_pragmas(&conn)?;
    Ok(conn)
}

fn apply_pragmas(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=OFF;
         PRAGMA foreign_keys=ON;
         PRAGMA cache_size=-64000;",
    )?;
    Ok(())
}

/// Creates the core tables (WITHOUT indexes — deferred for bulk-insert performance).
pub fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS scan_meta (
            id                INTEGER PRIMARY KEY,
            root_path         TEXT NOT NULL,
            scanned_at        INTEGER NOT NULL,
            total_files       INTEGER NOT NULL DEFAULT 0,
            total_dirs        INTEGER NOT NULL DEFAULT 0,
            total_size        INTEGER NOT NULL DEFAULT 0,
            scan_duration_ms  INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS dirs (
            id                  INTEGER PRIMARY KEY,
            parent_id           INTEGER REFERENCES dirs(id),
            name                TEXT NOT NULL,
            file_count          INTEGER NOT NULL DEFAULT 0,
            total_file_count    INTEGER NOT NULL DEFAULT 0,
            total_logical_size  INTEGER NOT NULL DEFAULT 0,
            total_disk_size     INTEGER NOT NULL DEFAULT 0,
            created_at          INTEGER,
            modified_at         INTEGER
        );

        CREATE TABLE IF NOT EXISTS files (
            id            INTEGER PRIMARY KEY,
            dir_id        INTEGER NOT NULL REFERENCES dirs(id),
            name          TEXT NOT NULL,
            logical_size  INTEGER NOT NULL DEFAULT 0,
            disk_size     INTEGER NOT NULL DEFAULT 0,
            created_at    INTEGER,
            modified_at   INTEGER,
            extension     TEXT,
            inode         INTEGER,
            content_hash  TEXT
        );",
    )?;
    Ok(())
}

/// Creates indexes after bulk insert for maximum ingestion throughput.
pub fn create_indexes(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_files_disk_size    ON files(disk_size DESC);
         CREATE INDEX IF NOT EXISTS idx_files_created_at   ON files(created_at);
         CREATE INDEX IF NOT EXISTS idx_files_modified_at  ON files(modified_at);
         CREATE INDEX IF NOT EXISTS idx_files_extension    ON files(extension);
         CREATE INDEX IF NOT EXISTS idx_files_dir_id       ON files(dir_id);
         CREATE INDEX IF NOT EXISTS idx_dirs_parent_id     ON dirs(parent_id);
         CREATE INDEX IF NOT EXISTS idx_dirs_disk_size     ON dirs(total_disk_size DESC);
         CREATE INDEX IF NOT EXISTS idx_files_content_hash ON files(content_hash) WHERE content_hash IS NOT NULL;",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Result<Connection> {
        let conn = open_memory_db()?;
        create_tables(&conn)?;
        Ok(conn)
    }

    #[test]
    fn test_tables_exist_after_creation() -> Result<()> {
        let conn = setup()?;

        let tables: Vec<String> = {
            let mut stmt = conn.prepare(
                "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name",
            )?;
            let result = stmt.query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            result
        };

        assert!(tables.contains(&"scan_meta".to_string()));
        assert!(tables.contains(&"dirs".to_string()));
        assert!(tables.contains(&"files".to_string()));
        Ok(())
    }

    #[test]
    fn test_insert_and_query_dir() -> Result<()> {
        let conn = setup()?;

        conn.execute(
            "INSERT INTO dirs (id, parent_id, name) VALUES (?1, ?2, ?3)",
            rusqlite::params![1i64, None::<i64>, "root"],
        )?;

        let name: String = conn.query_row(
            "SELECT name FROM dirs WHERE id = 1",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(name, "root");
        Ok(())
    }

    #[test]
    fn test_insert_and_query_file() -> Result<()> {
        let conn = setup()?;

        conn.execute(
            "INSERT INTO dirs (id, parent_id, name) VALUES (?1, ?2, ?3)",
            rusqlite::params![1i64, None::<i64>, "root"],
        )?;

        conn.execute(
            "INSERT INTO files (id, dir_id, name, logical_size, disk_size, extension)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![1i64, 1i64, "hello.txt", 100i64, 4096i64, "txt"],
        )?;

        let (name, disk_size): (String, i64) = conn.query_row(
            "SELECT name, disk_size FROM files WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        assert_eq!(name, "hello.txt");
        assert_eq!(disk_size, 4096);
        Ok(())
    }

    #[test]
    fn test_create_indexes() -> Result<()> {
        let conn = setup()?;
        create_indexes(&conn)?;

        let index_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name LIKE 'idx_%'",
            [],
            |row| row.get(0),
        )?;
        assert_eq!(index_count, 8);
        Ok(())
    }

    #[test]
    fn test_foreign_key_constraint() -> Result<()> {
        let conn = setup()?;

        // Inserting a file with a non-existent dir_id should fail because FK is ON.
        let result = conn.execute(
            "INSERT INTO files (id, dir_id, name) VALUES (1, 999, 'orphan.txt')",
            [],
        );
        assert!(result.is_err(), "foreign key constraint should reject invalid dir_id");
        Ok(())
    }
}
