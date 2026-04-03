use anyhow::{bail, Result};
use std::path::Path;

use crate::scanner::safety::is_dangerous_path;

// ---------------------------------------------------------------------------
// Result type
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DeleteResult {
    pub path: String,
    pub size_freed: u64,
    pub success: bool,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Size helpers
// ---------------------------------------------------------------------------

/// Calculate the total disk size of a path (file or directory tree).
fn calc_size(path: &Path) -> u64 {
    if path.is_file() {
        path.metadata().map(|m| m.len()).unwrap_or(0)
    } else if path.is_dir() {
        jwalk::WalkDir::new(path)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter_map(|e| e.metadata().ok().map(|m| m.len()))
            .sum()
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Move `path` to the system trash.
///
/// Returns an error if:
/// - The path is considered dangerous (matches system blocklist).
/// - The trash operation fails.
pub fn move_to_trash(path: &str) -> Result<DeleteResult> {
    let p = Path::new(path);

    if is_dangerous_path(p) {
        bail!("Refusing to trash dangerous path: {}", path);
    }

    let size_freed = calc_size(p);

    match trash::delete(p) {
        Ok(()) => Ok(DeleteResult {
            path: path.to_string(),
            size_freed,
            success: true,
            error: None,
        }),
        Err(e) => Ok(DeleteResult {
            path: path.to_string(),
            size_freed: 0,
            success: false,
            error: Some(e.to_string()),
        }),
    }
}

/// Permanently delete `path` (file or directory).
///
/// Returns an error if:
/// - The path is considered dangerous (matches system blocklist).
/// - The delete operation fails.
pub fn delete_permanent(path: &str) -> Result<DeleteResult> {
    let p = Path::new(path);

    if is_dangerous_path(p) {
        bail!("Refusing to permanently delete dangerous path: {}", path);
    }

    let size_freed = calc_size(p);

    let result = if p.is_dir() {
        std::fs::remove_dir_all(p)
    } else {
        std::fs::remove_file(p)
    };

    match result {
        Ok(()) => Ok(DeleteResult {
            path: path.to_string(),
            size_freed,
            success: true,
            error: None,
        }),
        Err(e) => Ok(DeleteResult {
            path: path.to_string(),
            size_freed: 0,
            success: false,
            error: Some(e.to_string()),
        }),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_dangerous_path_blocked_trash() {
        let err = move_to_trash("/").unwrap_err();
        assert!(
            err.to_string().contains("dangerous"),
            "expected dangerous path error, got: {}",
            err
        );
    }

    #[test]
    fn test_dangerous_path_blocked_permanent() {
        let err = delete_permanent("/System").unwrap_err();
        assert!(
            err.to_string().contains("dangerous"),
            "expected dangerous path error, got: {}",
            err
        );
    }

    #[test]
    fn test_permanent_delete_temp_file() {
        // Create a real temp file with some content
        let mut tmp = NamedTempFile::new().expect("create temp file");
        writeln!(tmp, "diskcopilot test data").expect("write to temp file");

        let path_str = tmp
            .path()
            .to_str()
            .expect("temp path is valid UTF-8")
            .to_string();

        // Forget the NamedTempFile so it doesn't try to delete a gone file
        let path_on_disk = tmp.into_temp_path();
        let path_str_clone = path_on_disk.to_str().unwrap().to_string();
        // Leak the TempPath — we'll delete it ourselves
        std::mem::forget(path_on_disk);

        let result = delete_permanent(&path_str_clone).expect("delete_permanent should return Ok");

        assert!(result.success, "expected success, got error: {:?}", result.error);
        assert_eq!(result.path, path_str_clone);
        assert!(result.size_freed > 0, "should have freed some bytes");
        assert!(
            !Path::new(&path_str_clone).exists(),
            "file should no longer exist"
        );

        // Suppress unused warning
        let _ = path_str;
    }

    #[test]
    fn test_short_path_blocked() {
        // /usr/bin has 3 components so it passes the component check
        // but /usr has 2 and is on the blocklist — use /tmp which is also blocked
        let err = delete_permanent("/tmp").unwrap_err();
        assert!(err.to_string().contains("dangerous"));
    }
}
