use anyhow::Result;
use rusqlite::Connection;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cache::reader::{
    query_dev_artifacts, query_large_files, query_old_files, query_recent_files, FileRow, TreeNode,
};
use crate::tui::app::{App, View};

// ---------------------------------------------------------------------------
// ViewConfig
// ---------------------------------------------------------------------------

/// Configurable thresholds for each list view.
pub struct ViewConfig {
    /// Minimum size in bytes to appear in Large Files view.
    pub large_file_threshold: u64,
    /// Files modified within this many days appear in Recent view.
    pub recent_days: u64,
    /// Files not modified for this many days appear in Old Files view.
    pub old_days: u64,
    /// Maximum rows to return for list views.
    pub limit: usize,
}

impl Default for ViewConfig {
    fn default() -> Self {
        Self {
            large_file_threshold: 10 * 1024 * 1024, // 10 MB
            recent_days: 7,
            old_days: 365,
            limit: 500,
        }
    }
}

// ---------------------------------------------------------------------------
// ViewData
// ---------------------------------------------------------------------------

/// The loaded data for a view.
pub enum ViewData {
    Tree,
    FileList(Vec<FileRow>),
    DirList(Vec<TreeNode>),
}

// ---------------------------------------------------------------------------
// Loader
// ---------------------------------------------------------------------------

/// Load data for the given view from the SQLite cache.
pub fn load_view_data(conn: &Connection, view: View, config: &ViewConfig) -> Result<ViewData> {
    match view {
        View::Tree => Ok(ViewData::Tree),

        View::LargeFiles => {
            let rows = query_large_files(conn, config.large_file_threshold, config.limit)?;
            Ok(ViewData::FileList(rows))
        }

        View::Recent => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let cutoff = now - (config.recent_days as i64 * 86400);
            let rows = query_recent_files(conn, cutoff, config.limit)?;
            Ok(ViewData::FileList(rows))
        }

        View::Old => {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let cutoff = now - (config.old_days as i64 * 86400);
            let rows = query_old_files(conn, cutoff, config.limit)?;
            Ok(ViewData::FileList(rows))
        }

        View::DevArtifacts => {
            let dirs = query_dev_artifacts(conn)?;
            Ok(ViewData::DirList(dirs))
        }

        View::Duplicates => {
            // Duplicate detection is Task 13; return empty for now.
            Ok(ViewData::FileList(Vec::new()))
        }
    }
}

// ---------------------------------------------------------------------------
// App helper
// ---------------------------------------------------------------------------

/// Switch the current view, load its data, and reset the cursor.
pub fn load_current_view(app: &mut App, conn: &Connection) -> Result<()> {
    let config = ViewConfig::default();
    match load_view_data(conn, app.view, &config)? {
        ViewData::Tree => {
            app.list_items.clear();
            app.rebuild_visible(conn)?;
        }
        ViewData::FileList(rows) => {
            app.list_items = rows;
        }
        ViewData::DirList(dirs) => {
            // Convert TreeNodes to FileRows for uniform display
            // (we reuse list_items which holds FileRow, so we map the key fields)
            app.list_items = dirs
                .into_iter()
                .map(|d| FileRow {
                    name: d.name,
                    full_path: String::new(), // path not available without reconstruction
                    disk_size: d.disk_size,
                    logical_size: d.logical_size,
                    created_at: d.created_at,
                    modified_at: d.modified_at,
                    extension: None,
                })
                .collect();
        }
    }
    app.cursor = 0;
    app.scroll_offset = 0;
    Ok(())
}
