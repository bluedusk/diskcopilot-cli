use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
};

use crate::format::format_size;
use crate::tui::app::App;

// ---------------------------------------------------------------------------
// StatusBar widget
// ---------------------------------------------------------------------------

/// Single-line status bar shown at the bottom of the screen.
///
/// Shows: total size scanned | file count | dir count | sort mode | help hint
pub struct StatusBar<'a> {
    app: &'a App,
}

impl<'a> StatusBar<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for StatusBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        let style = self.app.theme.status_bar;

        let line = if let Some(meta) = &self.app.scan_meta {
            let total_size = format_size(meta.total_size as u64);
            Line::from(vec![
                Span::styled(
                    format!(
                        " {}  {} files  {} dirs  sort: {}  ? help  q quit",
                        total_size,
                        meta.total_files,
                        meta.total_dirs,
                        self.app.sort_mode.label()
                    ),
                    style,
                ),
            ])
        } else {
            Line::from(vec![Span::styled(
                format!(" sort: {}  ? help  q quit", self.app.sort_mode.label()),
                style,
            )])
        };

        // Fill the entire row with the status bar background
        let blank = " ".repeat(area.width as usize);
        let bg_line = Line::from(Span::styled(blank, style));
        bg_line.render(area, buf);

        // Render the actual content on top
        line.render(area, buf);
    }
}
