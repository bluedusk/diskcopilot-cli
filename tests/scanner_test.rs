use diskcopilot::cache::schema::{create_indexes, create_tables, open_db};
use diskcopilot::scanner::walker::{scan_directory, ScanConfig, ScanProgress};

/// Integration test: create a small temp directory tree, scan it, and verify
/// the database contains the expected number of files and directories.
///
/// Tree:
///   <tmp>/
///     file_a.txt   (content: "hello")
///     file_b.rs    (content: "fn main() {}")
///     subdir/
///       file_c.md  (content: "# Title")
///
/// Expected after scan with full=true, min_file_size=0:
///   dirs  = 2  (root + subdir)
///   files = 3
#[test]
fn test_scan_full_counts() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path();

    std::fs::write(root.join("file_a.txt"), b"hello")?;
    std::fs::write(root.join("file_b.rs"), b"fn main() {}")?;
    let subdir = root.join("subdir");
    std::fs::create_dir(&subdir)?;
    std::fs::write(subdir.join("file_c.md"), b"# Title")?;

    // Use a temp file for the DB
    let db_file = tempfile::NamedTempFile::new()?;
    let db_path = db_file.path().to_path_buf();
    // Close the temp file handle so open_db can write to it
    drop(db_file);

    let mut conn = open_db(&db_path)?;
    create_tables(&conn)?;

    let progress = ScanProgress::new();
    let config = ScanConfig {
        full: true,
        min_file_size: 0,
    };

    {
        let mut writer =
            diskcopilot::cache::writer::CacheWriter::new(&mut conn, 1000);
        scan_directory(root, &config, &mut writer, &progress)?;
        writer.finalize()?;
    }

    create_indexes(&conn)?;

    // Verify via progress counters
    assert_eq!(progress.files(), 3, "expected 3 files");
    assert_eq!(progress.dirs(), 2, "expected 2 dirs");

    // Verify via database
    let file_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    let dir_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM dirs", [], |r| r.get(0))?;

    assert_eq!(file_count, 3, "DB should have 3 file rows");
    assert_eq!(dir_count, 2, "DB should have 2 dir rows");

    Ok(())
}

/// Verify that hard-linked files (same inode) are only stored once.
#[test]
fn test_hardlink_dedup() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path();

    let original = root.join("original.txt");
    std::fs::write(&original, b"shared content")?;
    // Create a hard link to the same inode
    std::fs::hard_link(&original, root.join("hardlink.txt"))?;

    let db_file = tempfile::NamedTempFile::new()?;
    let db_path = db_file.path().to_path_buf();
    drop(db_file);

    let mut conn = open_db(&db_path)?;
    create_tables(&conn)?;

    let progress = ScanProgress::new();
    let config = ScanConfig {
        full: true,
        min_file_size: 0,
    };

    {
        let mut writer =
            diskcopilot::cache::writer::CacheWriter::new(&mut conn, 1000);
        scan_directory(root, &config, &mut writer, &progress)?;
        writer.finalize()?;
    }

    // Only one of the two hard-linked entries should be stored in the DB
    let file_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get(0))?;
    assert_eq!(
        file_count,
        1,
        "hard-linked files should be deduplicated to 1 in DB"
    );

    Ok(())
}
