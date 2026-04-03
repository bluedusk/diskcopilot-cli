use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::format::format_size;
use crate::tui::app::{App, View};
use crate::tui::icons;

// ---------------------------------------------------------------------------
// Timestamp helper
// ---------------------------------------------------------------------------

/// Format a Unix timestamp (seconds) as "YYYY-MM-DD", or "-" if None.
fn format_timestamp(ts: Option<i64>) -> String {
    let Some(secs) = ts else {
        return "-".to_string();
    };
    // Simple calendar calculation without any external crate
    // Days since Unix epoch (1970-01-01)
    let days = (secs / 86400) as u64;
    let (year, month, day) = days_to_ymd(days);
    format!("{:04}-{:02}-{:02}", year, month, day)
}

/// Convert days since 1970-01-01 to (year, month, day) using the civil calendar algorithm.
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Shift epoch: 1970-01-01 = day 719468 in the 0000-03-01 Gregorian epoch
    let z = days + 719468;
    let era = z / 146097;
    let doe = z % 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

// ---------------------------------------------------------------------------
// Detail pane widget
// ---------------------------------------------------------------------------

/// Right-hand detail pane showing information about the selected item.
pub struct DetailPane<'a> {
    app: &'a App,
}

impl<'a> DetailPane<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for DetailPane<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .title(" Detail ")
            .borders(Borders::ALL)
            .border_style(self.app.theme.border);

        let inner = block.inner(area);
        block.render(area, buf);

        let lines = self.build_lines();
        let para = Paragraph::new(lines).style(self.app.theme.fg);
        para.render(inner, buf);
    }
}

impl DetailPane<'_> {
    fn build_lines(&self) -> Vec<Line<'static>> {
        match self.app.view {
            View::Tree => self.tree_detail_lines(),
            _ => self.list_detail_lines(),
        }
    }

    fn tree_detail_lines(&self) -> Vec<Line<'static>> {
        let Some(item) = self.app.visible_items.get(self.app.cursor) else {
            return vec![Line::from(" No item selected")];
        };

        let node = &item.node;
        let icon = icons::icon_for(&node.name, node.is_dir, item.is_expanded);
        let kind_label = if node.is_dir { "Directory" } else { "File" };

        let ratio = if item.parent_size > 0 {
            node.disk_size as f64 / item.parent_size as f64
        } else {
            0.0
        };

        let mut lines = vec![
            Line::from(vec![Span::styled(
                format!("{}{}", icon, node.name.clone()),
                if node.is_dir {
                    self.app.theme.dir
                } else {
                    self.app.theme.file_style(node.disk_size, item.parent_size)
                },
            )]),
            Line::from(""),
            label_value("Type", kind_label),
            label_value("Disk size", &format_size(node.disk_size)),
            label_value("Logical size", &format_size(node.logical_size)),
        ];

        if node.is_dir {
            lines.push(label_value("File count", &node.file_count.to_string()));
        }

        if let Some(ext) = &node.extension {
            lines.push(label_value("Extension", ext));
        }

        lines.push(label_value(
            "Modified",
            &format_timestamp(node.modified_at),
        ));
        lines.push(label_value(
            "Created",
            &format_timestamp(node.created_at),
        ));
        lines.push(label_value(
            "% of parent",
            &format!("{:.1}%", ratio * 100.0),
        ));

        lines
    }

    fn list_detail_lines(&self) -> Vec<Line<'static>> {
        let Some(row) = self.app.list_items.get(self.app.cursor) else {
            return vec![Line::from(" No item selected")];
        };

        let icon = icons::icon_for(&row.name, false, false);

        vec![
            Line::from(vec![Span::styled(
                format!("{}{}", icon, row.name.clone()),
                self.app.theme.file_large,
            )]),
            Line::from(""),
            label_value("Path", &row.full_path),
            label_value("Disk size", &format_size(row.disk_size)),
            label_value("Logical size", &format_size(row.logical_size)),
            label_value("Modified", &format_timestamp(row.modified_at)),
            label_value("Created", &format_timestamp(row.created_at)),
            if let Some(ext) = &row.extension {
                label_value("Extension", ext)
            } else {
                Line::from("")
            },
        ]
    }
}

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn label_value(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!(" {:<12} ", label),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::raw(value.to_string()),
    ])
}
