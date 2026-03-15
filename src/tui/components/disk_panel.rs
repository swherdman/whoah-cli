use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Gauge, Paragraph};

use crate::action::Action;
use crate::config::Thresholds;
use crate::ops::status::HostStatus;

use super::Component;

pub struct DiskPanel {
    status: Option<HostStatus>,
    thresholds: Thresholds,
    scroll: u16,
}

impl DiskPanel {
    pub fn new(thresholds: Thresholds) -> Self {
        Self {
            status: None,
            thresholds,
            scroll: 0,
        }
    }
}

impl Component for DiskPanel {
    fn update(&mut self, action: &Action) {
        match action {
            Action::UpdateStatus(status) => {
                self.status = Some(*status.clone());
            }
            Action::ScrollUp => {
                self.scroll = self.scroll.saturating_sub(1);
            }
            Action::ScrollDown => {
                self.scroll = self.scroll.saturating_add(1);
            }
            _ => {}
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" Disk Usage ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(status) = &self.status else {
            frame.render_widget(
                Paragraph::new("Loading...").style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        };

        // Calculate how many rows we need for layout
        let mut constraints: Vec<Constraint> = Vec::new();

        // rpool gauge (2 lines: label + gauge)
        if status.disk.rpool.is_some() {
            constraints.push(Constraint::Length(1)); // label
            constraints.push(Constraint::Length(1)); // gauge
        }

        // oxp pool gauges
        for _ in &status.disk.oxp_pools {
            constraints.push(Constraint::Length(1)); // label
            constraints.push(Constraint::Length(1)); // gauge
        }

        // Spacer
        constraints.push(Constraint::Length(1));

        // vdev files
        for _ in &status.disk.vdev_files {
            constraints.push(Constraint::Length(1));
        }

        // Fill remaining
        constraints.push(Constraint::Min(0));

        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        let mut row_idx = 0;

        // rpool
        if let Some(rpool) = &status.disk.rpool {
            let free_gib = rpool.free_bytes as f64 / 1_073_741_824.0;
            let color = gauge_color(rpool.capacity_pct, self.thresholds.rpool_warning_percent, self.thresholds.rpool_critical_percent);

            let label = format!("rpool: {}% ({:.0} GiB free)", rpool.capacity_pct, free_gib);
            frame.render_widget(
                Paragraph::new(label).style(Style::default().fg(color)),
                rows[row_idx],
            );
            row_idx += 1;

            let gauge = Gauge::default()
                .ratio(rpool.capacity_pct as f64 / 100.0)
                .gauge_style(Style::default().fg(color).bg(Color::DarkGray));
            frame.render_widget(gauge, rows[row_idx]);
            row_idx += 1;
        }

        // oxp pools
        for pool in &status.disk.oxp_pools {
            let short = pool
                .name
                .strip_prefix("oxp_")
                .map(|s| if s.len() > 8 { format!("oxp_{}...", &s[..8]) } else { format!("oxp_{s}") })
                .unwrap_or_else(|| pool.name.clone());

            let color = gauge_color(pool.capacity_pct, self.thresholds.oxp_pool_warning_percent, 95);

            let label = format!("{short}: {}%", pool.capacity_pct);
            frame.render_widget(
                Paragraph::new(label).style(Style::default().fg(color)),
                rows[row_idx],
            );
            row_idx += 1;

            let gauge = Gauge::default()
                .ratio(pool.capacity_pct as f64 / 100.0)
                .gauge_style(Style::default().fg(color).bg(Color::DarkGray));
            frame.render_widget(gauge, rows[row_idx]);
            row_idx += 1;
        }

        // Spacer
        row_idx += 1;

        // Vdev files
        for vdev in &status.disk.vdev_files {
            if row_idx >= rows.len() - 1 {
                break;
            }
            let name = vdev.path.rsplit('/').next().unwrap_or(&vdev.path);
            let gib = vdev.size_bytes as f64 / 1_073_741_824.0;
            let color = if gib > self.thresholds.vdev_warning_gib as f64 {
                Color::Red
            } else {
                Color::White
            };
            frame.render_widget(
                Paragraph::new(format!("{name}: {gib:.1} GiB")).style(Style::default().fg(color)),
                rows[row_idx],
            );
            row_idx += 1;
        }
    }
}

fn gauge_color(pct: u8, warning: u8, critical: u8) -> Color {
    if pct >= critical {
        Color::Red
    } else if pct >= warning {
        Color::Yellow
    } else {
        Color::Green
    }
}
