use diskcopilot::cache::reader::{load_children, load_root, query_large_files};
use diskcopilot::cache::schema::{create_indexes, create_tables, open_memory_db};
use diskcopilot::cache::writer::CacheWriter;
use diskcopilot::scanner::walker::{scan_directory, ScanConfig, ScanProgress};

/// Integration test: scan a temp directory tree, then use cache reader to verify
/// the round-trip: load_root, load_children, query_large_files, and size rollup.
///
/// Tree:
///   <tmp>/
///     small.txt       (11 bytes: "hello world")
///     subdir/
///       medium.txt    (5 bytes: "world")
///       big.bin       (2 MB of zeros)
#[test]
fn test_scan_and_read_roundtrip() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path();

    // Build the directory tree
    std::fs::write(root.join("small.txt"), b"hello world")?;
    let subdir = root.join("subdir");
    std::fs::create_dir(&subdir)?;
    std::fs::write(subdir.join("medium.txt"), b"world")?;
    let big_data = vec![0u8; 2 * 1024 * 1024]; // 2 MB
    std::fs::write(subdir.join("big.bin"), &big_data)?;

    // Scan into an in-memory DB
    let mut conn = open_memory_db()?;
    create_tables(&conn)?;

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
    create_indexes(&conn)?;

    // --- load_root ---
    let root_node = load_root(&conn)?;
    // The root name should be the directory name (last component of the temp path)
    let expected_root_name = root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(root_node.name, expected_root_name, "root name should match temp dir name");
    assert!(root_node.is_dir);

    // --- load_children of root ---
    let root_children = load_children(&conn, root_node.id)?;
    let child_names: Vec<&str> = root_children.iter().map(|c| c.name.as_str()).collect();
    assert!(
        child_names.contains(&"small.txt"),
        "root children should contain small.txt, got: {:?}",
        child_names
    );
    assert!(
        child_names.contains(&"subdir"),
        "root children should contain subdir, got: {:?}",
        child_names
    );
    assert_eq!(root_children.len(), 2, "root should have 2 children (small.txt + subdir)");

    // --- load_children of subdir ---
    let subdir_node = root_children.iter().find(|c| c.name == "subdir").unwrap();
    let subdir_children = load_children(&conn, subdir_node.id)?;
    let subdir_child_names: Vec<&str> = subdir_children.iter().map(|c| c.name.as_str()).collect();
    assert!(subdir_child_names.contains(&"medium.txt"));
    assert!(subdir_child_names.contains(&"big.bin"));
    assert_eq!(subdir_children.len(), 2);

    // --- query_large_files ---
    const MB: u64 = 1024 * 1024;
    let large_files = query_large_files(&conn, MB, 100)?;
    assert_eq!(large_files.len(), 1, "only big.bin should be >= 1 MB");
    assert_eq!(large_files[0].name, "big.bin");
    assert!(
        large_files[0].disk_size >= MB,
        "big.bin disk_size should be >= 1 MB"
    );

    // --- dir sizes rolled up correctly ---
    // subdir should include the sizes of medium.txt + big.bin
    assert!(
        subdir_node.disk_size > 0,
        "subdir disk_size should be > 0 after rollup"
    );
    // root should include everything (small.txt + subdir's rolled-up total)
    assert!(
        root_node.disk_size >= subdir_node.disk_size,
        "root disk_size ({}) should be >= subdir disk_size ({})",
        root_node.disk_size,
        subdir_node.disk_size,
    );
    // root file_count should be 3 (all files in the tree)
    assert_eq!(
        root_node.file_count, 3,
        "root total_file_count should be 3 (small.txt + medium.txt + big.bin)"
    );

    Ok(())
}
