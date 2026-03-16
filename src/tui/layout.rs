use ratatui::prelude::*;

pub struct DashboardAreas {
    pub title_bar: Rect,
    pub left_panel: Rect,
    pub right_panel: Rect,
    pub log_panel: Rect,
}

pub fn dashboard_layout(area: Rect) -> DashboardAreas {
    // Vertical: status info(1) | main content | logs
    // Tab bar and keybindings are rendered by the outer screen chrome.
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // status info (host + alerts)
            Constraint::Min(8),   // main content (services + disk)
            Constraint::Length(8), // log panel
        ])
        .split(area);

    // Main content: left (services/zones) | right (disk)
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(45),
            Constraint::Percentage(55),
        ])
        .split(vertical[1]);

    DashboardAreas {
        title_bar: vertical[0],
        left_panel: horizontal[0],
        right_panel: horizontal[1],
        log_panel: vertical[2],
    }
}
