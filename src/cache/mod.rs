pub mod schema;
pub mod writer;
pub mod reader;

use anyhow::Result;
use std::path::{Path, PathBuf};

/// Returns the diskcopilot cache directory: `~/.diskcopilot/cache/`.
pub fn cache_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("cannot determine home directory"))?;
    Ok(home.join(".diskcopilot").join("cache"))
}

/// Returns a deterministic DB path for a given root path, e.g.
/// `~/.diskcopilot/cache/<blake3-of-root>.db`.
pub fn db_path_for(root: &Path) -> Result<PathBuf> {
    let root_str = root
        .canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .to_string_lossy()
        .into_owned();

    let hash = blake3::hash(root_str.as_bytes());
    let filename = format!("{}.db", hash.to_hex());

    Ok(cache_dir()?.join(filename))
}
