pub mod app;
pub mod detail;
pub mod event;
pub mod icons;
pub mod search;
pub mod statusbar;
pub mod tabs;
pub mod theme;
pub mod tree;
pub mod views;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use app::{App, View};
use detail::DetailPane;
use statusbar::StatusBar;
use tabs::TabBar;
use tree::TreeView;

use crate::format::format_size;

// ---------------------------------------------------------------------------
// Top-level render function
// ---------------------------------------------------------------------------

/// Top-level render function. Called every frame.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Vertical layout: tab bar (1) | main content (fill) | status bar (1)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tab bar
            Constraint::Min(1),    // main content
            Constraint::Length(1), // status bar
        ])
        .split(area);

    // -----------------------------------------------------------------------
    // Tab bar
    // -----------------------------------------------------------------------
    frame.render_widget(TabBar::new(app), chunks[0]);

    // -----------------------------------------------------------------------
    // Main content area — optionally split with detail pane
    // -----------------------------------------------------------------------
    let main_area = chunks[1];

    if app.show_detail {
        let h_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(65),
                Constraint::Percentage(35),
            ])
            .split(main_area);

        render_primary(frame, app, h_chunks[0]);
        frame.render_widget(DetailPane::new(app), h_chunks[1]);
    } else {
        render_primary(frame, app, main_area);
    }

    // -----------------------------------------------------------------------
    // Status bar / Search bar
    // -----------------------------------------------------------------------
    if app.search.active {
        render_search_bar(frame, app, chunks[2]);
    } else {
        frame.render_widget(StatusBar::new(app), chunks[2]);
    }

    // -----------------------------------------------------------------------
    // Overlays
    // -----------------------------------------------------------------------
    if app.show_help {
        render_help(frame, app);
    }

    if let Some((idx, _)) = app.confirm_delete {
        render_confirm_delete(frame, app, idx);
    }
}

// ---------------------------------------------------------------------------
// Primary panel (tree or list)
// ---------------------------------------------------------------------------

fn render_primary(frame: &mut Frame, app: &App, area: Rect) {
    if app.view == View::Tree {
        frame.render_widget(TreeView::new(app), area);
    } else {
        render_list_view(frame, app, area);
    }
}

// ---------------------------------------------------------------------------
// List view (non-tree views)
// ---------------------------------------------------------------------------

/// Render list views: Large Files, Recent, Old, Dev Artifacts, Duplicates.
pub fn render_list_view(frame: &mut Frame, app: &App, area: Rect) {
    let title = format!(" {} ", app.view.label());
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(app.theme.border);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let items = &app.list_items;

    if items.is_empty() {
        let msg = match app.view {
            View::LargeFiles => " No large files found (threshold: 10 MB)",
            View::Recent => " No recently modified files found",
            View::Old => " No old files found",
            View::DevArtifacts => " No dev artifact directories found",
            View::Duplicates => " Duplicate detection not yet available",
            View::Tree => " ",
        };
        let line = Line::from(Span::styled(msg, app.theme.file_small));
        frame.render_widget(
            Paragraph::new(line),
            Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: 1,
            },
        );
        return;
    }

    let visible_height = inner.height as usize;
    let scroll = app.scroll_offset;
    let end = (scroll + visible_height).min(items.len());

    for (row_idx, item_idx) in (scroll..end).enumerate() {
        let row = &items[item_idx];
        let is_selected = item_idx == app.cursor;
        let is_marked = app.marked.contains(&item_idx);
        let row_y = inner.y + row_idx as u16;

        // Icon
        let icon = icons::icon_for(&row.name, false, false);

        // Mark indicator
        let mark = if is_marked { "● " } else { "  " };

        // Size string
        let size_str = format_size(row.disk_size);

        // Style
        let style = if is_selected {
            app.theme.selected
        } else {
            app.theme.file
        };

        // Right column: size (fixed width) + path (truncated)
        let path_display = if row.full_path.is_empty() {
            row.name.clone()
        } else {
            row.full_path.clone()
        };

        // Build row: mark + icon + name + padding + size
        // Format: "  icon name                  1.2 MB  /full/path"
        let size_col = format!("{:>9}", size_str);
        let avail = inner.width as usize;

        // Name section: mark(2) + icon(2) + name
        let name_part = format!("{}{}{}", mark, icon, row.name);
        // Right section: size + "  " + path
        let right_part = format!("  {}  {}", size_col, path_display);

        let left_max = avail.saturating_sub(right_part.len());
        let name_display = if name_part.len() > left_max {
            let mut s = name_part[..left_max.saturating_sub(1)].to_string();
            s.push('…');
            s
        } else {
            format!("{:<width$}", name_part, width = left_max)
        };

        // Path truncation for right part too
        let right_display = if right_part.len() > avail.saturating_sub(name_display.len()) {
            let available = avail.saturating_sub(name_display.len());
            if available > 0 {
                let r = &right_part[..available.min(right_part.len())];
                r.to_string()
            } else {
                String::new()
            }
        } else {
            right_part
        };

        let full_row = format!("{}{}", name_display, right_display);

        // Pad to full width
        let full_row = format!("{:<width$}", full_row, width = avail);

        let line = Line::from(Span::styled(full_row, style));
        frame.render_widget(
            Paragraph::new(line),
            Rect {
                x: inner.x,
                y: row_y,
                width: inner.width,
                height: 1,
            },
        );
    }
}

// ---------------------------------------------------------------------------
// Help overlay
// ---------------------------------------------------------------------------

fn render_help(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let popup_width = 54u16.min(area.width.saturating_sub(4));
    let popup_height = 24u16.min(area.height.saturating_sub(4));
    let popup_area = Rect {
        x: (area.width.saturating_sub(popup_width)) / 2,
        y: (area.height.saturating_sub(popup_height)) / 2,
        width: popup_width,
        height: popup_height,
    };

    frame.render_widget(Clear, popup_area);

    let help_text = vec![
        Line::from(Span::styled(
            "  Keyboard Shortcuts",
            Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )),
        Line::from(""),
        Line::from("  j / ↓         Move down"),
        Line::from("  k / ↑         Move up"),
        Line::from("  l / → / Enter  Expand directory"),
        Line::from("  h / ←         Collapse / go to parent"),
        Line::from("  g              Jump to top"),
        Line::from("  G              Jump to bottom"),
        Line::from("  Ctrl-d         Page down (20 rows)"),
        Line::from("  Ctrl-u         Page up (20 rows)"),
        Line::from(""),
        Line::from("  Tab            Next view"),
        Line::from("  Shift-Tab      Previous view"),
        Line::from("  s              Cycle sort mode"),
        Line::from("  Space          Mark / unmark item"),
        Line::from("  d              Delete (with confirmation)"),
        Line::from("  i              Toggle detail panel"),
        Line::from("  ?              This help screen"),
        Line::from("  q              Quit"),
        Line::from(""),
        Line::from(Span::styled(
            "  Press any key to close",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let help_block = Block::default()
        .title(" Help ")
        .borders(Borders::ALL)
        .border_style(app.theme.border)
        .style(app.theme.bg);

    let help_widget = Paragraph::new(help_text)
        .block(help_block)
        .style(app.theme.fg);

    frame.render_widget(help_widget, popup_area);
}

// ---------------------------------------------------------------------------
// Confirm delete overlay
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Search bar
// ---------------------------------------------------------------------------

fn render_search_bar(frame: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 {
        return;
    }

    let match_count = app.search.results.len();
    let right_label = if app.search.query.is_empty() {
        String::new()
    } else {
        format!("  {} match{}", match_count, if match_count == 1 { "" } else { "es" })
    };

    // Display: "/ <query>█" followed by match count on the right
    let query_part = format!("/ {}", app.search.query);
    let cursor_part = "█";

    let search_style = Style::default().fg(Color::Yellow);
    let count_style = Style::default().fg(Color::DarkGray);

    // Fill background
    let blank = " ".repeat(area.width as usize);
    let bg_line = Line::from(Span::styled(blank, search_style));
    use ratatui::widgets::Widget;
    bg_line.render(area, frame.buffer_mut());

    let line = Line::from(vec![
        Span::styled(query_part, search_style.add_modifier(Modifier::BOLD)),
        Span::styled(cursor_part, search_style.add_modifier(Modifier::REVERSED)),
        Span::styled(right_label, count_style),
    ]);

    Paragraph::new(line).render(area, frame.buffer_mut());
}

// ---------------------------------------------------------------------------
// Confirm delete overlay
// ---------------------------------------------------------------------------

fn render_confirm_delete(frame: &mut Frame, app: &App, item_idx: usize) {
    let area = frame.area();

    // Get item name for display
    let item_name = if app.view == View::Tree {
        app.visible_items
            .get(item_idx)
            .map(|i| i.node.name.as_str())
            .unwrap_or("(unknown)")
    } else {
        app.list_items
            .get(item_idx)
            .map(|r| r.name.as_str())
            .unwrap_or("(unknown)")
    };

    let popup_width = 58u16.min(area.width.saturating_sub(4));
    let popup_height = 8u16;
    let popup_area = Rect {
        x: (area.width.saturating_sub(popup_width)) / 2,
        y: (area.height.saturating_sub(popup_height)) / 2,
        width: popup_width,
        height: popup_height,
    };

    frame.render_widget(Clear, popup_area);

    // Truncate long names
    let max_name_len = (popup_width as usize).saturating_sub(6);
    let display_name = if item_name.len() > max_name_len {
        format!("{}…", &item_name[..max_name_len.saturating_sub(1)])
    } else {
        item_name.to_string()
    };

    let confirm_text = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  Delete: {}?", display_name),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  [t] Move to Trash    [d] Delete Permanently",
            Style::default().fg(Color::Yellow),
        )),
        Line::from(Span::styled(
            "  [n / Esc] Cancel",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
    ];

    let confirm_block = Block::default()
        .title(" Confirm Delete ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red))
        .style(app.theme.bg);

    let confirm_widget = Paragraph::new(confirm_text)
        .block(confirm_block)
        .style(app.theme.fg);

    frame.render_widget(confirm_widget, popup_area);
}
