use ratatui::style::{Color, Modifier, Style};

// ---------------------------------------------------------------------------
// Theme struct
// ---------------------------------------------------------------------------

/// All the styles used throughout the TUI.
#[derive(Debug, Clone)]
pub struct Theme {
    pub bg: Style,
    pub fg: Style,
    pub dir: Style,
    pub file: Style,
    pub file_large: Style,
    pub file_medium: Style,
    pub file_small: Style,
    pub selected: Style,
    pub bar_low: Style,
    pub bar_mid: Style,
    pub bar_high: Style,
    pub tab_active: Style,
    pub tab_inactive: Style,
    pub status_bar: Style,
    pub header: Style,
    pub border: Style,
}

impl Theme {
    // -----------------------------------------------------------------------
    // Dark theme
    // -----------------------------------------------------------------------
    pub fn dark() -> Self {
        Self {
            bg: Style::default().bg(Color::Black),
            fg: Style::default().fg(Color::White),
            dir: Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            file: Style::default().fg(Color::Gray),
            file_large: Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
            file_medium: Style::default().fg(Color::Yellow),
            file_small: Style::default().fg(Color::DarkGray),
            selected: Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
            bar_low: Style::default().fg(Color::Green),
            bar_mid: Style::default().fg(Color::Yellow),
            bar_high: Style::default().fg(Color::Red),
            tab_active: Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
            tab_inactive: Style::default().fg(Color::DarkGray),
            status_bar: Style::default().bg(Color::DarkGray).fg(Color::White),
            header: Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            border: Style::default().fg(Color::DarkGray),
        }
    }

    // -----------------------------------------------------------------------
    // Light theme
    // -----------------------------------------------------------------------
    pub fn light() -> Self {
        Self {
            bg: Style::default().bg(Color::White),
            fg: Style::default().fg(Color::Black),
            dir: Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD),
            file: Style::default().fg(Color::DarkGray),
            file_large: Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
            file_medium: Style::default().fg(Color::Rgb(180, 100, 0)),
            file_small: Style::default().fg(Color::Gray),
            selected: Style::default()
                .bg(Color::LightBlue)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
            bar_low: Style::default().fg(Color::Green),
            bar_mid: Style::default().fg(Color::Rgb(180, 100, 0)),
            bar_high: Style::default().fg(Color::Red),
            tab_active: Style::default()
                .fg(Color::White)
                .bg(Color::Blue)
                .add_modifier(Modifier::BOLD),
            tab_inactive: Style::default().fg(Color::Gray),
            status_bar: Style::default().bg(Color::LightBlue).fg(Color::Black),
            header: Style::default()
                .fg(Color::Blue)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            border: Style::default().fg(Color::Gray),
        }
    }

    // -----------------------------------------------------------------------
    // By name
    // -----------------------------------------------------------------------
    pub fn by_name(name: &str) -> Self {
        match name {
            "light" => Self::light(),
            _ => Self::dark(),
        }
    }

    // -----------------------------------------------------------------------
    // Dynamic helpers
    // -----------------------------------------------------------------------

    /// Return the appropriate file style based on this file's share of its
    /// parent directory's total size.
    /// - ratio > 10% → large (red)
    /// - ratio > 1%  → medium (yellow)
    /// - else        → small (gray)
    pub fn file_style(&self, size: u64, parent_total: u64) -> Style {
        if parent_total == 0 {
            return self.file_small;
        }
        let ratio = size as f64 / parent_total as f64;
        if ratio > 0.10 {
            self.file_large
        } else if ratio > 0.01 {
            self.file_medium
        } else {
            self.file_small
        }
    }

    /// Return the bar color based on a 0.0–1.0 proportion.
    /// - ratio <= 0.5 → green (low)
    /// - ratio <= 0.8 → yellow (mid)
    /// - else         → red (high)
    pub fn bar_color(&self, ratio: f64) -> Style {
        if ratio <= 0.50 {
            self.bar_low
        } else if ratio <= 0.80 {
            self.bar_mid
        } else {
            self.bar_high
        }
    }
}
