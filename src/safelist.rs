use anyhow::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Returns the safelist file path: `~/.diskcopilot/safelist.txt`
fn safelist_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
    Ok(home.join(".diskcopilot").join("safelist.txt"))
}

/// Load all safelist entries as canonical paths.
pub fn load() -> Result<HashSet<PathBuf>> {
    let path = safelist_path()?;
    if !path.exists() {
        return Ok(HashSet::new());
    }
    let content = std::fs::read_to_string(&path)?;
    let set = content
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|l| PathBuf::from(l.trim()))
        .collect();
    Ok(set)
}

/// Add a path to the safelist.
pub fn add(path: &Path) -> Result<()> {
    let canonical = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf());

    let mut entries = load()?;
    if !entries.insert(canonical.clone()) {
        anyhow::bail!("Already in safelist: {}", canonical.display());
    }
    write_entries(&entries)?;
    Ok(())
}

/// Remove a path from the safelist.
pub fn remove(path: &Path) -> Result<()> {
    let canonical = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf());

    let mut entries = load()?;
    if !entries.remove(&canonical) {
        // Also try the raw path in case canonicalization differs
        if !entries.remove(path) {
            anyhow::bail!("Not in safelist: {}", path.display());
        }
    }
    write_entries(&entries)?;
    Ok(())
}

/// Check if a path (or any of its ancestors) is in the safelist.
pub fn is_protected(path: &Path) -> bool {
    let entries = match load() {
        Ok(e) => e,
        Err(err) => {
            eprintln!(
                "Warning: failed to load safelist, treating '{}' as protected: {}",
                path.display(),
                err
            );
            return true;
        }
    };
    let canonical = path
        .canonicalize()
        .unwrap_or_else(|_| path.to_path_buf());

    // Check exact match
    if entries.contains(&canonical) || entries.contains(path) {
        return true;
    }

    // Check if any ancestor is protected
    let mut current = canonical.as_path();
    while let Some(parent) = current.parent() {
        if entries.contains(parent) {
            return true;
        }
        current = parent;
    }

    false
}

fn write_entries(entries: &HashSet<PathBuf>) -> Result<()> {
    let path = safelist_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut sorted: Vec<_> = entries.iter().collect();
    sorted.sort();
    let content: String = sorted
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, content + "\n")?;
    Ok(())
}
