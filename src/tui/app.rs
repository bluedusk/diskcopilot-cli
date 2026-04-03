use anyhow::Result;
use crossterm::{
    event::{Event as CEvent, EventStream, KeyCode, KeyEvent, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use futures::StreamExt;
use ratatui::{backend::CrosstermBackend, Terminal};
use rusqlite::Connection;
use std::collections::HashSet;
use std::io::stdout;
use std::time::Duration;
use tokio::time::interval;

use crate::cache::reader::{load_children, load_root, reconstruct_path, FileRow, TreeNode};
use crate::delete::trash::{delete_permanent, move_to_trash};
use crate::tui::{
    event::{channel, Event, EventReceiver, NEED_RENDER},
    search::SearchState,
    theme::Theme,
};

// ---------------------------------------------------------------------------
// View enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Tree,
    LargeFiles,
    Recent,
    Old,
    DevArtifacts,
    Duplicates,
}

impl View {
    pub fn all() -> &'static [View] {
        &[
            View::Tree,
            View::LargeFiles,
            View::Recent,
            View::Old,
            View::DevArtifacts,
            View::Duplicates,
        ]
    }

    pub fn label(&self) -> &'static str {
        match self {
            View::Tree => "Tree",
            View::LargeFiles => "Large Files",
            View::Recent => "Recent",
            View::Old => "Old Files",
            View::DevArtifacts => "Dev Artifacts",
            View::Duplicates => "Duplicates",
        }
    }

    pub fn next(&self) -> View {
        let all = Self::all();
        let idx = all.iter().position(|v| v == self).unwrap_or(0);
        all[(idx + 1) % all.len()]
    }

    pub fn prev(&self) -> View {
        let all = Self::all();
        let idx = all.iter().position(|v| v == self).unwrap_or(0);
        all[(idx + all.len() - 1) % all.len()]
    }
}

// ---------------------------------------------------------------------------
// SortMode enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortMode {
    SizeDesc,
    SizeAsc,
    Name,
    DateModified,
    DateCreated,
}

impl SortMode {
    pub fn next(&self) -> SortMode {
        match self {
            SortMode::SizeDesc => SortMode::SizeAsc,
            SortMode::SizeAsc => SortMode::Name,
            SortMode::Name => SortMode::DateModified,
            SortMode::DateModified => SortMode::DateCreated,
            SortMode::DateCreated => SortMode::SizeDesc,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            SortMode::SizeDesc => "Size ↓",
            SortMode::SizeAsc => "Size ↑",
            SortMode::Name => "Name",
            SortMode::DateModified => "Modified",
            SortMode::DateCreated => "Created",
        }
    }
}

// ---------------------------------------------------------------------------
// VisibleItem — a row in the tree view
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct VisibleItem {
    pub node: TreeNode,
    pub depth: usize,
    pub is_expanded: bool,
    pub parent_size: u64,
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

pub struct App {
    pub should_quit: bool,
    pub view: View,
    pub sort_mode: SortMode,
    pub theme: Theme,

    /// Root of the directory tree (None until loaded)
    pub root: Option<TreeNode>,

    /// Cursor position in the visible item list
    pub cursor: usize,
    pub scroll_offset: usize,

    /// Set of directory IDs that are currently expanded
    pub expanded: HashSet<i64>,

    pub show_detail: bool,
    pub show_help: bool,

    /// Flattened list of visible items in Tree view
    pub visible_items: Vec<VisibleItem>,

    /// List items for non-tree views (LargeFiles, Recent, etc.)
    pub list_items: Vec<FileRow>,

    /// Pending delete confirmation: holds (item_index, full_path) being confirmed
    pub confirm_delete: Option<(usize, String)>,

    /// Set of marked item indices (for multi-select)
    pub marked: HashSet<usize>,

    pub root_path: String,
    pub scan_meta: Option<ScanMeta>,

    /// Fuzzy search state
    pub search: SearchState,
}

#[derive(Debug, Clone)]
pub struct ScanMeta {
    pub scanned_at: i64,
    pub total_files: i64,
    pub total_dirs: i64,
    pub total_size: i64,
    pub scan_duration_ms: i64,
}

impl App {
    pub fn new(theme_name: &str) -> Self {
        Self {
            should_quit: false,
            view: View::Tree,
            sort_mode: SortMode::SizeDesc,
            theme: Theme::by_name(theme_name),
            root: None,
            cursor: 0,
            scroll_offset: 0,
            expanded: HashSet::new(),
            show_detail: false,
            show_help: false,
            visible_items: Vec::new(),
            list_items: Vec::new(),
            confirm_delete: None,
            marked: HashSet::new(),
            root_path: String::new(),
            scan_meta: None,
            search: SearchState::new(),
        }
    }

    /// Load the root node and top-level scan metadata from the SQLite cache.
    pub fn load_from_cache(&mut self, conn: &Connection) -> Result<()> {
        // Load root path and scan meta from the scan_meta table if available
        let meta_result: rusqlite::Result<(String, i64, i64, i64, i64, i64)> = conn.query_row(
            "SELECT root_path, scanned_at, total_files, total_dirs, total_size, scan_duration_ms
             FROM scan_meta LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        );

        if let Ok((root_path, scanned_at, total_files, total_dirs, total_size, scan_duration_ms)) =
            meta_result
        {
            self.root_path = root_path;
            self.scan_meta = Some(ScanMeta {
                scanned_at,
                total_files,
                total_dirs,
                total_size,
                scan_duration_ms,
            });
        }

        // Load the root tree node
        let root = load_root(conn)?;
        self.root_path = if self.root_path.is_empty() {
            root.name.clone()
        } else {
            self.root_path.clone()
        };
        self.root = Some(root);

        Ok(())
    }

    /// Rebuild the flat visible_items list from the current tree + expanded set.
    pub fn rebuild_visible(&mut self, conn: &Connection) -> Result<()> {
        self.visible_items.clear();

        let root = match &self.root {
            Some(r) => r.clone(),
            None => return Ok(()),
        };

        let root_size = root.disk_size;

        // Iterative DFS using a stack of (node, depth, parent_size)
        let mut stack: Vec<(TreeNode, usize, u64)> = vec![(root, 0, root_size)];

        while let Some((node, depth, parent_size)) = stack.pop() {
            let is_expanded = self.expanded.contains(&node.id);

            // If expanded and is a dir, push children (in reverse for correct order)
            if is_expanded && node.is_dir {
                let children = load_children(conn, node.id)?;
                let node_size = node.disk_size;
                let mut sorted = children;
                sort_nodes(&mut sorted, self.sort_mode);
                for child in sorted.into_iter().rev() {
                    stack.push((child, depth + 1, node_size));
                }
            }

            self.visible_items.push(VisibleItem {
                node,
                depth,
                is_expanded,
                parent_size,
            });
        }

        // Clamp cursor
        if !self.visible_items.is_empty() && self.cursor >= self.visible_items.len() {
            self.cursor = self.visible_items.len() - 1;
        }

        Ok(())
    }

    /// Move cursor by `delta` rows (positive = down, negative = up).
    /// Adjusts scroll_offset to keep the cursor visible in the viewport.
    pub fn move_cursor(&mut self, delta: i64) {
        let len = if self.view == View::Tree {
            self.visible_items.len()
        } else {
            self.list_items.len()
        };
        if len == 0 {
            return;
        }
        let new_cursor = (self.cursor as i64 + delta).clamp(0, len as i64 - 1) as usize;
        self.cursor = new_cursor;

        // Keep cursor visible: scroll up or down as needed
        let visible_height: usize = 40;
        if self.cursor < self.scroll_offset {
            self.scroll_offset = self.cursor;
        } else if self.cursor >= self.scroll_offset + visible_height {
            self.scroll_offset = self.cursor + 1 - visible_height;
        }
    }

    /// Toggle expansion of the item at the current cursor position.
    pub fn toggle_expand(&mut self, conn: &Connection) -> Result<()> {
        if self.view != View::Tree {
            return Ok(());
        }
        if let Some(item) = self.visible_items.get(self.cursor) {
            if item.node.is_dir {
                let id = item.node.id;
                if self.expanded.contains(&id) {
                    self.expanded.remove(&id);
                } else {
                    self.expanded.insert(id);
                }
                self.rebuild_visible(conn)?;
            }
        }
        Ok(())
    }

    /// Collapse the current node, or jump to its parent if already collapsed.
    pub fn collapse_or_parent(&mut self, conn: &Connection) -> Result<()> {
        if self.view != View::Tree {
            return Ok(());
        }

        if let Some(item) = self.visible_items.get(self.cursor).cloned() {
            if item.is_expanded && item.node.is_dir {
                // Collapse current dir
                self.expanded.remove(&item.node.id);
                self.rebuild_visible(conn)?;
            } else if item.depth > 0 {
                // Find the parent (first item with depth == current depth - 1 above cursor)
                let target_depth = item.depth - 1;
                for i in (0..self.cursor).rev() {
                    if self.visible_items[i].depth == target_depth {
                        self.cursor = i;
                        break;
                    }
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Sorting helper
// ---------------------------------------------------------------------------

fn sort_nodes(nodes: &mut Vec<TreeNode>, mode: SortMode) {
    match mode {
        SortMode::SizeDesc => nodes.sort_by(|a, b| b.disk_size.cmp(&a.disk_size)),
        SortMode::SizeAsc => nodes.sort_by(|a, b| a.disk_size.cmp(&b.disk_size)),
        SortMode::Name => nodes.sort_by(|a, b| a.name.cmp(&b.name)),
        SortMode::DateModified => nodes.sort_by(|a, b| b.modified_at.cmp(&a.modified_at)),
        SortMode::DateCreated => nodes.sort_by(|a, b| b.created_at.cmp(&a.created_at)),
    }
}

// ---------------------------------------------------------------------------
// Key handler
// ---------------------------------------------------------------------------

pub fn handle_key(app: &mut App, key: KeyEvent, conn: &Connection) -> Result<()> {
    // Handle confirmation dialog first
    if let Some((_, ref full_path)) = app.confirm_delete.clone() {
        match key.code {
            KeyCode::Char('d') | KeyCode::Char('D') => {
                // Permanent delete
                let _ = delete_permanent(full_path);
                app.confirm_delete = None;
                app.load_from_cache(conn)?;
                app.rebuild_visible(conn)?;
            }
            KeyCode::Char('t') | KeyCode::Char('T') => {
                // Move to trash
                let _ = move_to_trash(full_path);
                app.confirm_delete = None;
                app.load_from_cache(conn)?;
                app.rebuild_visible(conn)?;
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                app.confirm_delete = None;
            }
            _ => {}
        }
        return Ok(());
    }

    if app.show_help {
        app.show_help = false;
        return Ok(());
    }

    // ------------------------------------------------------------------
    // Search mode — handle all keys while search is active
    // ------------------------------------------------------------------
    if app.search.active {
        match key.code {
            KeyCode::Esc => {
                app.search.deactivate();
            }
            KeyCode::Enter => {
                // Jump to first result and close search
                if let Some(result) = app.search.results.first() {
                    app.cursor = result.index;
                }
                app.search.deactivate();
            }
            KeyCode::Backspace => {
                app.search.pop_char();
                let items = search_items(app);
                app.search.filter(&items);
                if let Some(result) = app.search.results.first() {
                    app.cursor = result.index;
                }
            }
            KeyCode::Char(c) => {
                app.search.push_char(c);
                let items = search_items(app);
                app.search.filter(&items);
                if let Some(result) = app.search.results.first() {
                    app.cursor = result.index;
                }
            }
            _ => {}
        }
        return Ok(());
    }

    match key.code {
        // Quit
        KeyCode::Char('q') | KeyCode::Char('Q') => {
            app.should_quit = true;
        }

        // Navigation
        KeyCode::Char('j') | KeyCode::Down => {
            app.move_cursor(1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.move_cursor(-1);
        }

        // Expand / open
        KeyCode::Char('l') | KeyCode::Right | KeyCode::Enter => {
            app.toggle_expand(conn)?;
        }

        // Collapse / go up
        KeyCode::Char('h') | KeyCode::Left => {
            app.collapse_or_parent(conn)?;
        }

        // Jump to top / bottom
        KeyCode::Char('g') => {
            app.cursor = 0;
        }
        KeyCode::Char('G') => {
            let len = if app.view == View::Tree {
                app.visible_items.len()
            } else {
                app.list_items.len()
            };
            if len > 0 {
                app.cursor = len - 1;
            }
        }

        // Page down / up
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.move_cursor(20);
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.move_cursor(-20);
        }

        // Tab between views
        KeyCode::Tab => {
            app.view = app.view.next();
            crate::tui::views::load_current_view(app, conn)?;
        }
        KeyCode::BackTab => {
            app.view = app.view.prev();
            crate::tui::views::load_current_view(app, conn)?;
        }

        // Mark / unmark item
        KeyCode::Char(' ') => {
            if app.marked.contains(&app.cursor) {
                app.marked.remove(&app.cursor);
            } else {
                app.marked.insert(app.cursor);
            }
        }

        // Delete with confirmation
        KeyCode::Char('d') => {
            let cursor = app.cursor;
            let full_path = if app.view == View::Tree {
                app.visible_items.get(cursor).and_then(|item| {
                    if item.node.is_dir {
                        reconstruct_path(conn, item.node.id).ok()
                    } else {
                        // For files: reconstruct parent dir path, then append filename
                        // Files are stored under a parent dir; we need the parent's id.
                        // The TreeNode for a file doesn't carry its dir_id directly,
                        // so we fall back to name-only as a best-effort path.
                        Some(item.node.name.clone())
                    }
                })
            } else {
                app.list_items
                    .get(cursor)
                    .map(|row| row.full_path.clone())
            };
            if let Some(path) = full_path {
                app.confirm_delete = Some((cursor, path));
            }
        }

        // Sort cycle
        KeyCode::Char('s') => {
            app.sort_mode = app.sort_mode.next();
            let _ = app.rebuild_visible(conn);
        }

        // Toggle detail panel
        KeyCode::Char('i') => {
            app.show_detail = !app.show_detail;
        }

        // Help overlay
        KeyCode::Char('?') => {
            app.show_help = true;
        }

        // Fuzzy search
        KeyCode::Char('/') => {
            app.search.activate();
        }

        _ => {}
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helper: build a (index, name) slice for the current view's items
// ---------------------------------------------------------------------------

fn search_items(app: &App) -> Vec<(usize, String)> {
    if app.view == View::Tree {
        app.visible_items
            .iter()
            .enumerate()
            .map(|(i, item)| (i, item.node.name.clone()))
            .collect()
    } else {
        app.list_items
            .iter()
            .enumerate()
            .map(|(i, row)| (i, row.name.clone()))
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Main async run function
// ---------------------------------------------------------------------------

pub async fn run(conn: Connection, theme_name: &str) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    let result = run_inner(&mut terminal, conn, theme_name).await;

    // Always clean up
    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;

    result
}

async fn run_inner(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    conn: Connection,
    theme_name: &str,
) -> Result<()> {
    let (tx, mut rx): (_, EventReceiver) = channel();

    // Spawn crossterm event reader task
    let tx_clone = tx.clone();
    tokio::spawn(async move {
        let mut reader = EventStream::new();
        while let Some(event_result) = reader.next().await {
            match event_result {
                Ok(CEvent::Key(key)) => {
                    let _ = tx_clone.send(Event::Key(key));
                }
                Ok(CEvent::Resize(w, h)) => {
                    let _ = tx_clone.send(Event::Resize(w, h));
                }
                _ => {}
            }
        }
    });

    // Tick timer — 10 ms
    let tx_tick = tx.clone();
    tokio::spawn(async move {
        let mut ticker = interval(Duration::from_millis(10));
        loop {
            ticker.tick().await;
            if tx_tick.send(Event::Tick).is_err() {
                break;
            }
        }
    });

    let mut app = App::new(theme_name);
    app.load_from_cache(&conn)?;
    app.rebuild_visible(&conn)?;

    // Render debounce: we render at most once per 10 ms
    let mut render_pending = true;

    loop {
        // Drain all pending events before rendering
        let event = tokio::time::timeout(Duration::from_millis(10), rx.recv()).await;

        match event {
            Ok(Some(Event::Key(key))) => {
                handle_key(&mut app, key, &conn)?;
                render_pending = true;
            }
            Ok(Some(Event::Resize(_, _))) => {
                render_pending = true;
            }
            Ok(Some(Event::Tick)) => {
                // Check NEED_RENDER flag
                let flag = NEED_RENDER.swap(0, std::sync::atomic::Ordering::Relaxed);
                if flag > 0 {
                    render_pending = true;
                }
            }
            Ok(Some(_)) => {
                render_pending = true;
            }
            Ok(None) => break, // channel closed
            Err(_) => {
                // Timeout — render if pending
            }
        }

        if render_pending {
            terminal.draw(|frame| {
                crate::tui::render(frame, &app);
            })?;
            render_pending = false;
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}
