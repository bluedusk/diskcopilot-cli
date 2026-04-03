#[cfg(target_os = "macos")]
use std::os::darwin::fs::MetadataExt as DarwinMetadataExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

/// File metadata extracted from the filesystem.
#[derive(Debug, Clone)]
pub struct FileMeta {
    /// Logical (reported) file size in bytes.
    pub logical_size: u64,
    /// Actual disk usage in bytes (blocks * 512).
    pub disk_size: u64,
    /// Creation time as Unix timestamp (seconds). Maps to ctime on Linux,
    /// btime on macOS when available via MetadataExt::ctime.
    pub created_at: Option<i64>,
    /// Modification time as Unix timestamp (seconds).
    pub modified_at: Option<i64>,
    /// Inode number.
    pub inode: u64,
    /// Whether this entry is a directory.
    pub is_dir: bool,
    /// Whether this entry is a symbolic link.
    pub is_symlink: bool,
}

/// Extract metadata for a path using `symlink_metadata` (does not follow symlinks).
pub fn extract_metadata(path: &Path) -> anyhow::Result<FileMeta> {
    let m = path.symlink_metadata()?;
    let file_type = m.file_type();

    let logical_size = m.len();
    // blocks() returns 512-byte blocks on macOS/Linux
    let disk_size = m.blocks() * 512;
    let inode = m.ino();
    let is_dir = file_type.is_dir();
    let is_symlink = file_type.is_symlink();

    // mtime is universally available via MetadataExt
    let modified_at = Some(m.mtime());

    // On macOS, use st_birthtime (true file creation time) instead of ctime
    // (which is inode change time, not creation time).
    #[cfg(target_os = "macos")]
    let created_at = Some(m.st_birthtime() as i64);
    #[cfg(not(target_os = "macos"))]
    let created_at = Some(m.ctime());

    Ok(FileMeta {
        logical_size,
        disk_size,
        created_at,
        modified_at,
        inode,
        is_dir,
        is_symlink,
    })
}

/// Return the lowercase file extension from a filename, or `None`.
///
/// Examples: `"photo.JPG"` → `Some("jpg")`, `"Makefile"` → `None`.
pub fn file_extension(name: &str) -> Option<String> {
    let name = name.trim_end_matches('/');
    let dot_pos = name.rfind('.')?;
    // Dot at position 0 means hidden file with no extension (.bashrc)
    // Dot as last char means no extension (file.)
    if dot_pos == 0 || dot_pos == name.len() - 1 {
        return None;
    }
    Some(name[dot_pos + 1..].to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write as _;

    #[test]
    fn test_extract_metadata_file() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("hello.txt");
        let mut f = fs::File::create(&path)?;
        f.write_all(b"hello world")?;
        drop(f);

        let meta = extract_metadata(&path)?;
        assert_eq!(meta.logical_size, 11);
        assert!(!meta.is_dir);
        assert!(!meta.is_symlink);
        assert!(meta.inode > 0);
        assert!(meta.modified_at.is_some());
        assert!(meta.created_at.is_some());
        // disk_size should be a multiple of 512 (at least one block)
        assert_eq!(meta.disk_size % 512, 0);
        Ok(())
    }

    #[test]
    fn test_extract_metadata_dir() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let meta = extract_metadata(dir.path())?;
        assert!(meta.is_dir);
        assert!(!meta.is_symlink);
        Ok(())
    }

    #[test]
    fn test_extract_metadata_symlink() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let target = dir.path().join("target.txt");
        fs::write(&target, b"data")?;
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link)?;

        let meta = extract_metadata(&link)?;
        assert!(meta.is_symlink);
        assert!(!meta.is_dir);
        Ok(())
    }

    #[test]
    fn test_file_extension() {
        assert_eq!(file_extension("photo.JPG"), Some("jpg".into()));
        assert_eq!(file_extension("archive.tar.gz"), Some("gz".into()));
        assert_eq!(file_extension("Makefile"), None);
        assert_eq!(file_extension(".bashrc"), None);
        assert_eq!(file_extension("file."), None);
        assert_eq!(file_extension("doc.PDF"), Some("pdf".into()));
        assert_eq!(file_extension("no_ext"), None);
    }
}
