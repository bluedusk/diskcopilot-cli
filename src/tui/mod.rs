pub mod app;
pub mod event;
pub mod icons;
pub mod theme;

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use app::{App, View};

/// Top-level render function. Called every frame.
///
/// Currently a placeholder that shows the root path, active view, and item
/// count. Task 9 will implement the full tree view rendering.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Divide screen into: header (1) | tabs (1) | body (fill) | status (1)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // header
            Constraint::Length(1), // tab bar
            Constraint::Min(1),    // body
            Constraint::Length(1), // status bar
        ])
        .split(area);

    // -----------------------------------------------------------------------
    // Header
    // -----------------------------------------------------------------------
    let header_text = format!(" DiskCopilot  {}", app.root_path);
    let header = Paragraph::new(header_text)
        .style(app.theme.header)
        .alignment(Alignment::Left);
    frame.render_widget(header, chunks[0]);

    // -----------------------------------------------------------------------
    // Tab bar
    // -----------------------------------------------------------------------
    let tab_spans: Vec<Span> = View::all()
        .iter()
        .flat_map(|v| {
            let style = if *v == app.view {
                app.theme.tab_active
            } else {
                app.theme.tab_inactive
            };
            vec![
                Span::styled(format!(" {} ", v.label()), style),
                Span::raw(" "),
            ]
        })
        .collect();
    let tabs = Paragraph::new(Line::from(tab_spans));
    frame.render_widget(tabs, chunks[1]);

    // -----------------------------------------------------------------------
    // Body — placeholder content
    // -----------------------------------------------------------------------
    let item_count = if app.view == View::Tree {
        app.visible_items.len()
    } else {
        app.list_items.len()
    };

    let sort_label = app.sort_mode.label();
    let body_text = vec![
        Line::from(vec![
            Span::styled("View: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(app.view.label()),
        ]),
        Line::from(vec![
            Span::styled("Sort: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(sort_label),
        ]),
        Line::from(vec![
            Span::styled("Items: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(item_count.to_string()),
        ]),
        Line::from(""),
        Line::from(vec![Span::styled(
            "Press ? for help, q to quit",
            Style::default().fg(Color::DarkGray),
        )]),
    ];

    let body_block = Block::default()
        .borders(Borders::ALL)
        .border_style(app.theme.border)
        .title(format!(" {} ", app.view.label()));

    let body = Paragraph::new(body_text)
        .block(body_block)
        .style(app.theme.fg);

    frame.render_widget(body, chunks[2]);

    // -----------------------------------------------------------------------
    // Help overlay
    // -----------------------------------------------------------------------
    if app.show_help {
        render_help(frame, app);
    }

    // -----------------------------------------------------------------------
    // Status bar
    // -----------------------------------------------------------------------
    let status_text = if let Some(meta) = &app.scan_meta {
        format!(
            " {} files  {} dirs  cursor: {}  sort: {}",
            meta.total_files, meta.total_dirs, app.cursor, sort_label
        )
    } else {
        format!(" cursor: {}  sort: {}", app.cursor, sort_label)
    };
    let status = Paragraph::new(status_text).style(app.theme.status_bar);
    frame.render_widget(status, chunks[3]);
}

// ---------------------------------------------------------------------------
// Help overlay
// ---------------------------------------------------------------------------

fn render_help(frame: &mut Frame, app: &App) {
    use ratatui::{
        layout::Rect,
        widgets::Clear,
    };

    let area = frame.area();
    // Center a 50x20 popup
    let popup_width = 52u16.min(area.width.saturating_sub(4));
    let popup_height = 22u16.min(area.height.saturating_sub(4));
    let popup_area = Rect {
        x: (area.width.saturating_sub(popup_width)) / 2,
        y: (area.height.saturating_sub(popup_height)) / 2,
        width: popup_width,
        height: popup_height,
    };

    frame.render_widget(Clear, popup_area);

    let help_text = vec![
        Line::from(Span::styled(
            "Keyboard Shortcuts",
            Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )),
        Line::from(""),
        Line::from("  j / ↓       Move down"),
        Line::from("  k / ↑       Move up"),
        Line::from("  l / → / ↵   Expand directory"),
        Line::from("  h / ←       Collapse / go to parent"),
        Line::from("  g            Jump to top"),
        Line::from("  G            Jump to bottom"),
        Line::from("  Ctrl-d       Page down"),
        Line::from("  Ctrl-u       Page up"),
        Line::from(""),
        Line::from("  Tab          Next view"),
        Line::from("  Shift-Tab    Previous view"),
        Line::from("  s            Cycle sort mode"),
        Line::from("  Space        Mark / unmark item"),
        Line::from("  d            Delete (with confirm)"),
        Line::from("  i            Toggle detail panel"),
        Line::from("  ?            This help screen"),
        Line::from("  q            Quit"),
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
