use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::action::Action;
use crate::ops::status::HostStatus;
use crate::parse::services::ServiceState;

use super::Component;

pub struct StatusPanel {
    status: Option<HostStatus>,
    scroll: u16,
}

impl StatusPanel {
    pub fn new() -> Self {
        Self {
            status: None,
            scroll: 0,
        }
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
        let block = Block::default()
            .title(" Services ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let Some(status) = &self.status else {
            frame.render_widget(
                Paragraph::new("Connecting...").style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        };

        let mut lines: Vec<Line> = Vec::new();

        // Service states
        let sled_state = status.services.sled_agent.as_ref();
        let baseline_state = status.services.baseline.as_ref();

        lines.push(Line::from(vec![
            Span::raw("sled-agent: "),
            service_span(sled_state),
        ]));
        lines.push(Line::from(vec![
            Span::raw("baseline:   "),
            service_span(baseline_state),
        ]));

        // Per-service zone counts
        lines.push(Line::from(""));
        lines.push(Line::from(
            Span::styled("Zones", Style::default().add_modifier(Modifier::BOLD)),
        ));

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
                Some(exp) if actual >= exp => Color::Green,
                Some(_) => Color::Red,
                None => Color::White, // no expectation set
            };
            let count_str = match expected {
                Some(exp) => format!("{actual}/{exp}"),
                None => format!("{actual}"),
            };
            lines.push(Line::from(vec![
                Span::raw(format!("  {svc:<16} ")),
                Span::styled(count_str, Style::default().fg(color)),
            ]));
        }

        if status.zones.instance_count > 0 {
            lines.push(Line::from(vec![
                Span::raw(format!("  {:<16} ", "instances")),
                Span::styled(
                    format!("{}", status.zones.instance_count),
                    Style::default().fg(Color::Cyan),
                ),
            ]));
        }

        // Network
        let nexus_span = if status.network.nexus_reachable {
            Span::styled("reachable", Style::default().fg(Color::Green))
        } else {
            Span::styled("unreachable", Style::default().fg(Color::Red))
        };
        let dns_span = if status.network.dns_resolving {
            Span::styled("resolving", Style::default().fg(Color::Green))
        } else {
            Span::styled("not resolving", Style::default().fg(Color::Yellow))
        };
        lines.push(Line::from(vec![Span::raw("nexus:      "), nexus_span]));
        lines.push(Line::from(vec![Span::raw("dns:        "), dns_span]));

        // Zone placement
        if !status.zones.placement.is_empty() {
            lines.push(Line::from(""));
            lines.push(Line::from(
                Span::styled("Zone Placement", Style::default().add_modifier(Modifier::BOLD)),
            ));

            let mut pools: Vec<_> = status.zones.placement.iter().collect();
            pools.sort_by_key(|(k, _)| (*k).clone());
            for (pool, zone_names) in pools {
                let short = shorten_pool(pool);
                lines.push(Line::from(format!("  {short}: {}", zone_names.join(", "))));
            }
        }

        let paragraph = Paragraph::new(lines).scroll((self.scroll, 0));
        frame.render_widget(paragraph, inner);
    }
}

fn service_span(state: Option<&ServiceState>) -> Span<'static> {
    match state {
        Some(ServiceState::Online) => Span::styled("online", Style::default().fg(Color::Green)),
        Some(ServiceState::Maintenance) => {
            Span::styled("maintenance", Style::default().fg(Color::Red))
        }
        Some(ServiceState::Offline) => Span::styled("offline", Style::default().fg(Color::Yellow)),
        Some(ServiceState::Degraded) => {
            Span::styled("degraded", Style::default().fg(Color::Yellow))
        }
        Some(other) => Span::styled(other.to_string(), Style::default().fg(Color::DarkGray)),
        None => Span::styled("not found", Style::default().fg(Color::DarkGray)),
    }
}

fn shorten_pool(name: &str) -> String {
    name.strip_prefix("oxp_")
        .map(|s| {
            if s.len() > 8 {
                format!("oxp_{}...", &s[..8])
            } else {
                format!("oxp_{s}")
            }
        })
        .unwrap_or_else(|| name.to_string())
}
