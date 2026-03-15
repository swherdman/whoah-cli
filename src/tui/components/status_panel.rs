use std::collections::HashMap;

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::action::Action;
use crate::ops::status::HostStatus;
use crate::parse::services::ServiceState;
use crate::parse::zones::ZoneKind;
use crate::tui::theme::{self, Palette};

use super::Component;

pub struct StatusPanel {
    status: Option<HostStatus>,
    scroll: u16,
    focused: bool,
}

impl StatusPanel {
    pub fn new() -> Self {
        Self {
            status: None,
            scroll: 0,
            focused: false,
        }
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }
}

impl Component for StatusPanel {
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
        let block = theme::panel_block("Services", self.focused, &p);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(status) = &self.status else {
            frame.render_widget(
                Paragraph::new("Connecting...").style(Style::default().fg(p.text_disabled)),
                inner,
            );
            return;
        };

        let mut lines: Vec<Line> = Vec::new();

        // Service states
        let sled_state = status.services.sled_agent.as_ref();
        let baseline_state = status.services.baseline.as_ref();

        lines.push(Line::from(vec![
            Span::styled("sled-agent: ", Style::default().fg(p.text_raised)),
            service_span(sled_state, &p),
        ]));
        lines.push(Line::from(vec![
            Span::styled("baseline:   ", Style::default().fg(p.text_raised)),
            service_span(baseline_state, &p),
        ]));

        // Network
        let nexus_span = if status.network.nexus_reachable {
            Span::styled("reachable", Style::default().fg(p.green_primary))
        } else {
            Span::styled("unreachable", Style::default().fg(p.red_error))
        };
        let dns_span = if status.network.dns_resolving {
            Span::styled("resolving", Style::default().fg(p.green_primary))
        } else {
            Span::styled("not resolving", Style::default().fg(p.yellow_warn))
        };
        lines.push(Line::from(vec![
            Span::styled("nexus:      ", Style::default().fg(p.text_raised)),
            nexus_span,
        ]));
        lines.push(Line::from(vec![
            Span::styled("dns:        ", Style::default().fg(p.text_raised)),
            dns_span,
        ]));

        // Zone placement grid with ■/□
        if !status.zones.placement.is_empty() {
            lines.push(Line::from(""));
            lines.push(theme::section_header("Zone Distribution", &p));

            let placement = &status.zones.placement;
            let mut pool_names: Vec<&String> = placement.keys().collect();
            pool_names.sort();

            // Column headers: pool numbers
            let mut header_spans = vec![
                Span::styled(format!("{:<18} ", ""), Style::default().fg(p.text_tertiary)),
            ];
            for (i, _) in pool_names.iter().enumerate() {
                header_spans.push(Span::styled(
                    format!("{:<4}", i),
                    Style::default().fg(p.text_tertiary),
                ));
            }
            lines.push(Line::from(header_spans));

            // Build a grid: service_name -> [count_per_pool]
            let mut service_pool_counts: HashMap<String, Vec<u32>> = HashMap::new();
            let mut instance_pool_counts: Vec<u32> = vec![0; pool_names.len()];

            for (pool_idx, pool_name) in pool_names.iter().enumerate() {
                if let Some(zone_names) = placement.get(*pool_name) {
                    for zone_name in zone_names {
                        // Check if this is a propolis instance
                        let is_instance = status.zones.zones.iter().any(|z| {
                            z.service_name == *zone_name && z.kind == ZoneKind::Instance
                        });
                        if is_instance {
                            instance_pool_counts[pool_idx] += 1;
                        } else {
                            let counts = service_pool_counts
                                .entry(zone_name.clone())
                                .or_insert_with(|| vec![0; pool_names.len()]);
                            counts[pool_idx] += 1;
                        }
                    }
                }
            }

            // Render service rows
            let mut svc_names: Vec<&String> = service_pool_counts.keys().collect();
            svc_names.sort();
            for svc_name in svc_names {
                let counts = &service_pool_counts[svc_name];
                let mut row_spans = vec![
                    Span::styled(
                        format!("  {:<16} ", svc_name),
                        Style::default().fg(p.text_secondary),
                    ),
                ];
                for &count in counts {
                    let (ch, color) = if count == 0 {
                        ("\u{25A1}", p.text_disabled) // □
                    } else if count == 1 {
                        ("\u{25A0}", p.green_primary) // ■
                    } else {
                        // Use number for count > 1
                        ("", p.green_primary)
                    };
                    if count > 1 {
                        row_spans.push(Span::styled(
                            format!("{:<4}", count),
                            Style::default().fg(color),
                        ));
                    } else {
                        row_spans.push(Span::styled(
                            format!("{:<4}", ch),
                            Style::default().fg(color),
                        ));
                    }
                }
                lines.push(Line::from(row_spans));
            }

            // Render instance rows if any
            let has_instances = instance_pool_counts.iter().any(|&c| c > 0);
            if has_instances {
                lines.push(Line::from(""));
                lines.push(theme::section_header("Instances", &p));
                let mut row_spans = vec![
                    Span::styled(
                        format!("  {:<16} ", "propolis-server"),
                        Style::default().fg(p.text_secondary),
                    ),
                ];
                for &count in &instance_pool_counts {
                    let (ch, color) = if count == 0 {
                        ("\u{25A1}", p.text_disabled)
                    } else if count == 1 {
                        ("\u{25A0}", p.blue_info)
                    } else {
                        ("", p.blue_info)
                    };
                    if count > 1 {
                        row_spans.push(Span::styled(
                            format!("{:<4}", count),
                            Style::default().fg(color),
                        ));
                    } else {
                        row_spans.push(Span::styled(
                            format!("{:<4}", ch),
                            Style::default().fg(color),
                        ));
                    }
                }
                lines.push(Line::from(row_spans));
            }
        }

        let paragraph = Paragraph::new(lines).scroll((self.scroll, 0));
        frame.render_widget(paragraph, inner);
    }
}

/// Simple count-based zone display (e.g., "cockroachdb  3/3").
/// Retained for future use — currently replaced by the ■/□ grid view.
#[allow(dead_code)]
fn zone_count_lines(status: &HostStatus, p: &Palette) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(theme::section_header("Zones", p));

    let mut all_services: Vec<String> = status.zones.expected_services.keys().cloned().collect();
    for svc in status.zones.service_counts.keys() {
        if !all_services.contains(svc) {
            all_services.push(svc.clone());
        }
    }
    all_services.sort();

    for svc in &all_services {
        let actual = status.zones.service_counts.get(svc).copied().unwrap_or(0);
        let expected = status.zones.expected_services.get(svc).copied();
        let color = match expected {
            Some(exp) if actual >= exp => p.green_primary,
            Some(_) => p.red_error,
            None => p.text_default,
        };
        let count_str = match expected {
            Some(exp) => format!("{actual}/{exp}"),
            None => format!("{actual}"),
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {svc:<16} "), Style::default().fg(p.text_secondary)),
            Span::styled(count_str, Style::default().fg(color)),
        ]));
    }

    if status.zones.instance_count > 0 {
        lines.push(Line::from(vec![
            Span::styled(format!("  {:<16} ", "instances"), Style::default().fg(p.text_secondary)),
            Span::styled(
                format!("{}", status.zones.instance_count),
                Style::default().fg(p.blue_info),
            ),
        ]));
    }

    lines
}

fn service_span(state: Option<&ServiceState>, p: &Palette) -> Span<'static> {
    match state {
        Some(ServiceState::Online) => Span::styled("online", Style::default().fg(p.green_primary)),
        Some(ServiceState::Maintenance) => {
            Span::styled("maintenance", Style::default().fg(p.red_error))
        }
        Some(ServiceState::Offline) => Span::styled("offline", Style::default().fg(p.yellow_warn)),
        Some(ServiceState::Degraded) => {
            Span::styled("degraded", Style::default().fg(p.yellow_warn))
        }
        Some(other) => Span::styled(other.to_string(), Style::default().fg(p.text_disabled)),
        None => Span::styled("not found", Style::default().fg(p.text_disabled)),
    }
}
