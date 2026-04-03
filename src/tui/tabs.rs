use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::{Line, Span},
    widgets::Widget,
};

use crate::tui::app::{App, View};

// ---------------------------------------------------------------------------
// TabBar widget
// ---------------------------------------------------------------------------

/// A single-line tab bar that shows all views, highlighting the active one.
///
/// Renders as:  Tree │ Large Files │ Recent │ Old Files │ Dev Artifacts │ Duplicates
pub struct TabBar<'a> {
    app: &'a App,
}

impl<'a> TabBar<'a> {
    pub fn new(app: &'a App) -> Self {
        Self { app }
    }
}

impl Widget for TabBar<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 {
            return;
        }

        let mut spans: Vec<Span> = Vec::new();

        let all = View::all();
        for (i, view) in all.iter().enumerate() {
            let style = if *view == self.app.view {
                self.app.theme.tab_active
            } else {
                self.app.theme.tab_inactive
            };
            spans.push(Span::styled(format!(" {} ", view.label()), style));
            if i + 1 < all.len() {
                spans.push(Span::styled(" │ ", self.app.theme.tab_inactive));
            }
        }

        let line = Line::from(spans);
        line.render(area, buf);
    }
}
