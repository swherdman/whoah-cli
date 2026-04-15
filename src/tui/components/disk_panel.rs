use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::action::Action;
use crate::config::Thresholds;
use crate::ops::status::HostStatus;
use crate::tui::theme::{self, Palette};

use super::Component;

pub struct DiskPanel {
    status: Option<HostStatus>,
    thresholds: Thresholds,
    vdev_size_bytes: u64,
    scroll: u16,
    focused: bool,
}

impl DiskPanel {
    pub fn new(thresholds: Thresholds, vdev_size_bytes: u64) -> Self {
        Self {
            status: None,
            thresholds,
            vdev_size_bytes,
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

            lines.push(Line::from(vec![Span::styled(
                format!("rpool: {}% ({:.0} GiB free)", rpool.capacity_pct, free_gib),
                Style::default().fg(color),
            )]));
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

        // Vdev files — split by type (U.2 data drives vs M.2 boot drives)
        let u2_vdevs: Vec<_> = status
            .disk
            .vdev_files
            .iter()
            .filter(|v| v.path.contains("u2_"))
            .collect();
        let m2_vdevs: Vec<_> = status
            .disk
            .vdev_files
            .iter()
            .filter(|v| v.path.contains("m2_"))
            .collect();

        let logical_gib = self.vdev_size_bytes as f64 / 1_073_741_824.0;

        if !u2_vdevs.is_empty() {
            lines.push(Line::from(""));
            lines.push(theme::section_header("U.2 Data Drives", &p));
            for vdev in &u2_vdevs {
                let name = vdev.path.rsplit('/').next().unwrap_or(&vdev.path);
                let gib = vdev.size_bytes as f64 / 1_073_741_824.0;
                let pct = if self.vdev_size_bytes > 0 {
                    (gib / logical_gib * 100.0) as u8
                } else {
                    0
                };
                let color = theme::threshold_color(
                    pct,
                    ((self.thresholds.vdev_warning_gib as f64 / logical_gib) * 100.0) as u8,
                    95,
                    &p,
                );
                lines.push(Line::from(Span::styled(
                    format!("{name}: {gib:.1} / {logical_gib:.0} GiB ({pct}%)"),
                    Style::default().fg(color),
                )));
                lines.push(theme::render_bar(
                    gib / logical_gib,
                    bar_width,
                    color,
                    p.ascii_structural,
                ));
            }
        }

        if !m2_vdevs.is_empty() {
            lines.push(Line::from(""));
            lines.push(theme::section_header("M.2 Boot Drives", &p));
            for vdev in &m2_vdevs {
                let name = vdev.path.rsplit('/').next().unwrap_or(&vdev.path);
                let gib = vdev.size_bytes as f64 / 1_073_741_824.0;
                let pct = if self.vdev_size_bytes > 0 {
                    (gib / logical_gib * 100.0) as u8
                } else {
                    0
                };
                let color = theme::threshold_color(pct, 50, 80, &p);
                lines.push(Line::from(Span::styled(
                    format!("{name}: {gib:.1} / {logical_gib:.0} GiB ({pct}%)"),
                    Style::default().fg(color),
                )));
                lines.push(theme::render_bar(
                    gib / logical_gib,
                    bar_width,
                    color,
                    p.ascii_structural,
                ));
            }
        }

        let total_lines = lines.len();
        frame.render_widget(Paragraph::new(lines).scroll((self.scroll, 0)), inner);

        // Scrollbar
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .style(Style::default().fg(p.border_default));
        let mut scrollbar_state = ScrollbarState::new(total_lines).position(self.scroll as usize);
        frame.render_stateful_widget(scrollbar, inner, &mut scrollbar_state);
    }
}
