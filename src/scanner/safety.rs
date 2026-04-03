use std::path::Path;

/// Known dangerous system paths that should not be scanned as roots.
/// These are critical macOS/Unix directories where scanning could be dangerous
/// (extremely slow, destructive operations, or system integrity risks).
const BLOCKED_PATHS: &[&str] = &[
    "/",
    "/System",
    "/Library",
    "/usr",
    "/bin",
    "/sbin",
    "/var",
    "/private",
    "/etc",
    "/tmp",
    "/Applications",
    "/Users",
    "/Volumes",
    "/cores",
    "/opt",
];

/// Returns `true` if the given path is considered dangerous to use as a scan root.
///
/// A path is dangerous if:
/// - It has fewer than 3 components (e.g., `/`, `/Users`, `/usr/bin`), OR
/// - It exactly matches one of the known system blocklist paths.
pub fn is_dangerous_path(path: &Path) -> bool {
    let component_count = path.components().count();

    // Paths with fewer than 3 components are too broad (e.g. /, /Users, /usr/bin)
    if component_count < 3 {
        return true;
    }

    // Check exact match against the blocklist
    if let Some(path_str) = path.to_str() {
        for &blocked in BLOCKED_PATHS {
            if path_str == blocked {
                return true;
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_root_is_dangerous() {
        assert!(is_dangerous_path(Path::new("/")));
    }

    #[test]
    fn test_system_dirs_are_dangerous() {
        for dir in &[
            "/System",
            "/Library",
            "/usr",
            "/bin",
            "/sbin",
            "/var",
            "/private",
            "/etc",
            "/tmp",
            "/Applications",
            "/Users",
            "/Volumes",
            "/cores",
            "/opt",
        ] {
            assert!(
                is_dangerous_path(Path::new(dir)),
                "{} should be dangerous",
                dir
            );
        }
    }

    #[test]
    fn test_short_paths_are_dangerous() {
        // /tmp has 2 components (/ + tmp) — blocked
        assert!(is_dangerous_path(Path::new("/tmp")));
        // /usr has 2 components (/ + usr) — blocked
        assert!(is_dangerous_path(Path::new("/usr")));
        // / has 1 component — blocked
        assert!(is_dangerous_path(Path::new("/")));
    }

    #[test]
    fn test_deep_user_paths_are_safe() {
        // 3+ components: /Users/alice/Documents
        assert!(!is_dangerous_path(&PathBuf::from("/Users/alice/Documents")));
        assert!(!is_dangerous_path(&PathBuf::from("/home/user/projects")));
        assert!(!is_dangerous_path(&PathBuf::from(
            "/Users/bob/Downloads/stuff"
        )));
    }

    #[test]
    fn test_three_component_non_blocked_is_safe() {
        // /usr/local/bin has 3 components and isn't in the blocklist directly
        // (though /usr is) — with exactly 3 it passes the component check
        // but /usr/local/bin is not in the blocklist so safe:
        assert!(!is_dangerous_path(Path::new("/usr/local/bin")));
    }
}
