use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Widget},
};

use crate::format::format_size;
use crate::tui::app::App;
use crate::tui::icons;

// ---------------------------------------------------------------------------
// TreeView widget
// ---------------------------------------------------------------------------

/// Renders the tree view: indent + arrow + icon + name + size bar.
pub struct TreeView<'a> {
    app: &'a App,
}

impl<'a> TreeView<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for TreeView<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(format!(" {} ", self.app.view.label()))
            .borders(Borders::ALL)
            .border_style(self.app.theme.border);

        let inner = block.inner(area);
        block.render(area, buf);

        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let visible_height = inner.height as usize;
        let items = &self.app.visible_items;

        if items.is_empty() {
            let empty = Line::from(Span::styled(" (empty)", self.app.theme.file_small));
            empty.render(
                Rect {
                    x: inner.x,
                    y: inner.y,
                    width: inner.width,
                    height: 1,
                },
                buf,
            );
            return;
        }

        // Determine scroll window
        let scroll = self.app.scroll_offset;
        let end = (scroll + visible_height).min(items.len());

        for (row_idx, item_idx) in (scroll..end).enumerate() {
            let item = &items[item_idx];
            let is_selected = item_idx == self.app.cursor;
            let is_marked = self.app.marked.contains(&item_idx);

            let row_y = inner.y + row_idx as u16;

            // Build row content
            // Indent: 2 spaces per depth level
            let indent = "  ".repeat(item.depth);

            // Arrow for directories
            let arrow = if item.node.is_dir {
                if item.is_expanded {
                    "▼ "
                } else {
                    "▶ "
                }
            } else {
                "  "
            };

            // Mark indicator
            let mark = if is_marked { "● " } else { "  " };

            // Icon
            let icon = icons::icon_for(&item.node.name, item.node.is_dir, item.is_expanded);

            // Size
            let size_str = format_size(item.node.disk_size);

            // File count suffix for dirs
            let count_str = if item.node.is_dir && item.node.file_count > 0 {
                format!(" ({})", item.node.file_count)
            } else {
                String::new()
            };

            // Percentage bar (8 chars wide)
            let ratio = if item.parent_size > 0 {
                (item.node.disk_size as f64 / item.parent_size as f64).min(1.0)
            } else {
                0.0
            };
            let bar = build_bar(ratio, 8);
            let bar_style = self.app.theme.bar_color(ratio);

            // Determine base text style
            let text_style = if is_selected {
                self.app.theme.selected
            } else if item.node.is_dir {
                self.app.theme.dir
            } else {
                self.app.theme.file_style(item.node.disk_size, item.parent_size)
            };

            // Size column style (keep color, but subdued when not selected)
            let size_style = if is_selected {
                self.app.theme.selected
            } else {
                self.app.theme.file_style(item.node.disk_size, item.parent_size)
            };

            // Build the left portion: indent + mark + arrow + icon + name + count
            let left = format!("{}{}{}{}{}{}", indent, mark, arrow, icon, item.node.name, count_str);

            // Right portion: size (right-aligned) + bar
            // Total available width for right part: ~20 chars
            let right = format!(" {:>9} {}", size_str, bar);

            // Calculate widths
            let avail_width = inner.width as usize;
            let right_width = right.len();
            let left_max = avail_width.saturating_sub(right_width);

            // Truncate left if needed
            let left_display = if left.len() > left_max {
                let mut s = left[..left_max.saturating_sub(1)].to_string();
                s.push('…');
                s
            } else {
                // Pad to fill space
                format!("{:<width$}", left, width = left_max)
            };

            // Fill background for selected row
            if is_selected {
                let blank = " ".repeat(avail_width);
                let blank_line = Line::from(Span::styled(blank, self.app.theme.selected));
                blank_line.render(
                    Rect {
                        x: inner.x,
                        y: row_y,
                        width: inner.width,
                        height: 1,
                    },
                    buf,
                );
            }

            // Render left portion (name)
            let name_span = Span::styled(left_display, text_style);
            let name_line = Line::from(name_span);
            name_line.render(
                Rect {
                    x: inner.x,
                    y: row_y,
                    width: left_max as u16,
                    height: 1,
                },
                buf,
            );

            // Render size
            let size_part = format!(" {:>9} ", size_str);
            let size_part_len = size_part.len();
            let bar_x = inner.x + left_max as u16 + size_part_len as u16;
            let size_span = Span::styled(size_part, size_style);
            let size_line = Line::from(size_span);
            size_line.render(
                Rect {
                    x: inner.x + left_max as u16,
                    y: row_y,
                    width: (right_width as u16).min(inner.width.saturating_sub(left_max as u16)),
                    height: 1,
                },
                buf,
            );

            // Render bar
            let bar_width = inner
                .width
                .saturating_sub(left_max as u16)
                .saturating_sub(size_part_len as u16);
            if bar_width > 0 {
                let bar_style_final = if is_selected {
                    // Merge bar color fg with selected background
                    Style::default()
                        .fg(bar_style.fg.unwrap_or(ratatui::style::Color::Green))
                        .bg(self.app.theme.selected.bg.unwrap_or(ratatui::style::Color::DarkGray))
                } else {
                    bar_style
                };
                let bar_span = Span::styled(bar, bar_style_final);
                let bar_line = Line::from(bar_span);
                bar_line.render(
                    Rect {
                        x: bar_x,
                        y: row_y,
                        width: bar_width,
                        height: 1,
                    },
                    buf,
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Bar helper
// ---------------------------------------------------------------------------

/// Build a simple Unicode block bar of `width` characters representing `ratio` (0.0–1.0).
fn build_bar(ratio: f64, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let filled = ((ratio * width as f64).round() as usize).min(width);
    let empty = width - filled;
    let mut s = String::with_capacity(width);
    for _ in 0..filled {
        s.push('█');
    }
    for _ in 0..empty {
        s.push('░');
    }
    s
}
