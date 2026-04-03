use diskcopilot::cache::reader::{load_root, load_scan_meta, load_tree_to_depth, query_large_files};
use diskcopilot::cache::schema::{create_indexes, create_tables, open_memory_db};
use diskcopilot::cache::writer::{CacheWriter, ScanMeta};
use diskcopilot::scanner::walker::{scan_directory, ScanConfig, ScanProgress};

#[test]
fn test_query_json_roundtrip() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let root = tmp.path();

    // Create test files
    let big_data = vec![0u8; 2 * 1024 * 1024];
    std::fs::write(root.join("big.bin"), &big_data)?;
    std::fs::write(root.join("small.txt"), b"hello")?;

    // Scan
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
        writer.write_meta(&ScanMeta {
            root_path: root.to_string_lossy().into_owned(),
            scanned_at: 1000,
            total_files: progress.files() as i64,
            total_dirs: progress.dirs() as i64,
            total_size: progress.size() as i64,
            scan_duration_ms: 100,
        })?;
    }
    create_indexes(&conn)?;

    // Test large files query + JSON serialization
    let large = query_large_files(&conn, 1024 * 1024, 10)?;
    let json = serde_json::to_string_pretty(&large)?;
    assert!(json.contains("big.bin"));
    // Deserialize back
    let parsed: Vec<serde_json::Value> = serde_json::from_str(&json)?;
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0]["name"], "big.bin");

    // Test tree JSON
    let root_node = load_root(&conn)?;
    let tree = load_tree_to_depth(&conn, root_node.id, 1)?;
    let tree_json = serde_json::to_string_pretty(&tree)?;
    assert!(tree_json.contains("big.bin"));
    assert!(tree_json.contains("small.txt"));

    // Test scan meta JSON
    let meta = load_scan_meta(&conn)?.expect("should have meta");
    let meta_json = serde_json::to_string_pretty(&meta)?;
    assert!(meta_json.contains("root_path"));

    Ok(())
}
