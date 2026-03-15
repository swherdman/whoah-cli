use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::action::Action;
use crate::config::Thresholds;
use crate::ops::status::HostStatus;
use crate::tui::theme::{self, Palette};

use super::Component;

pub struct DiskPanel {
    status: Option<HostStatus>,
    thresholds: Thresholds,
    scroll: u16,
    focused: bool,
}

impl DiskPanel {
    pub fn new(thresholds: Thresholds) -> Self {
        Self {
            status: None,
            thresholds,
            scroll: 0,
            focused: false,
        }
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
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
        let p = Palette::default();
        let block = theme::panel_block("Disk Usage", self.focused, &p);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(status) = &self.status else {
            frame.render_widget(
                Paragraph::new("Loading...").style(Style::default().fg(p.text_disabled)),
                inner,
            );
            return;
        };

        let bar_width = inner.width;
        let mut lines: Vec<Line> = Vec::new();

        // rpool
        if let Some(rpool) = &status.disk.rpool {
            let free_gib = rpool.free_bytes as f64 / 1_073_741_824.0;
            let color = theme::threshold_color(
                rpool.capacity_pct,
                self.thresholds.rpool_warning_percent,
                self.thresholds.rpool_critical_percent,
                &p,
            );

            lines.push(Line::from(vec![
                Span::styled(
                    format!("rpool: {}% ({:.0} GiB free)", rpool.capacity_pct, free_gib),
                    Style::default().fg(color),
                ),
            ]));
            lines.push(theme::render_bar(
                rpool.capacity_pct as f64 / 100.0,
                bar_width,
                color,
                p.ascii_structural,
            ));
        }

        // oxp pools
        for pool in &status.disk.oxp_pools {
            let short = pool
                .name
                .strip_prefix("oxp_")
                .map(|s| {
                    if s.len() > 8 {
                        format!("oxp_{}...", &s[..8])
                    } else {
                        format!("oxp_{s}")
                    }
                })
                .unwrap_or_else(|| pool.name.clone());

            let color = theme::threshold_color(
                pool.capacity_pct,
                self.thresholds.oxp_pool_warning_percent,
                95,
                &p,
            );

            lines.push(Line::from(Span::styled(
                format!("{short}: {}%", pool.capacity_pct),
                Style::default().fg(color),
            )));
            lines.push(theme::render_bar(
                pool.capacity_pct as f64 / 100.0,
                bar_width,
                color,
                p.ascii_structural,
            ));
        }

        // Spacer
        if !status.disk.vdev_files.is_empty() {
            lines.push(Line::from(""));
            lines.push(theme::section_header("Vdevs", &p));
        }

        // Vdev files
        for vdev in &status.disk.vdev_files {
            let name = vdev.path.rsplit('/').next().unwrap_or(&vdev.path);
            let gib = vdev.size_bytes as f64 / 1_073_741_824.0;
            let color = if gib > self.thresholds.vdev_warning_gib as f64 {
                p.red_error
            } else {
                p.text_default
            };
            lines.push(Line::from(Span::styled(
                format!("{name}: {gib:.1} GiB"),
                Style::default().fg(color),
            )));
        }

        let paragraph = Paragraph::new(lines).scroll((self.scroll, 0));
        frame.render_widget(paragraph, inner);
    }
}
