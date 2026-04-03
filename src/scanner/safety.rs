use std::path::Path;

/// System paths where the directory itself AND all its subdirectories are dangerous.
/// Prefix matching is applied: any path under these roots is blocked.
/// Note: "/" is not listed here because it is already caught by the component-count
/// check (< 3 components), and including it would block every absolute path.
const DANGEROUS_ROOTS: &[&str] = &[
    "/System",
    "/Library",
    "/private",
];

/// Paths that are dangerous as exact scan targets but whose subdirectories may
/// be legitimate. These are also caught by the component-count check (< 3
/// components) but listed explicitly for documentation.
const BLOCKED_EXACT: &[&str] = &[
    "/usr",
    "/bin",
    "/sbin",
    "/var",
    "/etc",
    "/tmp",
    "/cores",
    "/opt",
    "/Applications",
    "/Users",
    "/Volumes",
];

/// Returns `true` if the given path is considered dangerous to use as a scan root.
///
/// A path is dangerous if:
/// - It has fewer than 3 components (e.g., `/`, `/Users`, `/usr/bin`), OR
/// - It or any of its ancestor prefixes matches one of the known system blocklist paths.
pub fn is_dangerous_path(path: &Path) -> bool {
    let component_count = path.components().count();

    // Paths with fewer than 3 components are too broad (e.g. /, /Users, /usr/bin)
    if component_count < 3 {
        return true;
    }

    // Check exact match against container directories
    if let Some(path_str) = path.to_str() {
        for &blocked in BLOCKED_EXACT {
            if path_str == blocked {
                return true;
            }
        }
    }

    // Check if any prefix of the path matches a dangerous root
    for ancestor in path.ancestors() {
        let ancestor_str = ancestor.to_string_lossy();
        for &dangerous in DANGEROUS_ROOTS {
            if ancestor_str.as_ref() == dangerous {
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
    fn test_subdirs_of_blocked_paths_are_dangerous() {
        // /System/Library/CoreServices is a subdirectory of /System, which is blocked
        assert!(is_dangerous_path(Path::new("/System/Library/CoreServices")));
        // /Library/Application Support is under /Library
        assert!(is_dangerous_path(Path::new("/Library/Application Support")));
        // /private/var/db is under /private, which is a dangerous root
        assert!(is_dangerous_path(Path::new("/private/var/db")));
    }

    #[test]
    fn test_user_installed_paths_are_safe() {
        // /usr/local/bin has 4 components and /usr is only exact-blocked (not prefix-blocked)
        assert!(!is_dangerous_path(Path::new("/usr/local/bin")));
        // /var/folders is commonly used for temp files and should be safe
        assert!(!is_dangerous_path(Path::new("/var/folders/abc")));
    }

    #[test]
    fn test_paths_inside_dangerous_roots_blocked() {
        // /System is a DANGEROUS_ROOT — anything under it should be blocked
        assert!(is_dangerous_path(Path::new("/System/Library/CoreServices")));
        // /Library is a DANGEROUS_ROOT — sub-paths should be blocked
        assert!(is_dangerous_path(Path::new(
            "/Library/Frameworks/Something.framework"
        )));
    }

    #[test]
    fn test_users_subpaths_are_safe() {
        // /Users is in BLOCKED_EXACT (not prefix-matched via DANGEROUS_ROOTS),
        // so deep paths under /Users should be safe.
        assert!(!is_dangerous_path(Path::new("/Users/alice/Library/Caches")));
    }
}
