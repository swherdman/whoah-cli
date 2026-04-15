use std::time::Instant;

use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use crate::ssh::registry::{self, SessionSnapshot};
use crate::tui::theme::{Palette, format_duration, panel_block_accent};

use crate::action::Action;

use super::Component;

pub struct DebugView {
    sessions: Vec<SessionSnapshot>,
    containers: Vec<ContainerInfo>,
    scroll: u16,
    last_refresh: Option<Instant>,
}

pub struct ContainerInfo {
    pub name: String,
    pub status: String,
    pub ports: String,
}

impl DebugView {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            containers: Vec::new(),
            scroll: 0,
            last_refresh: None,
        }
    }

    /// Refresh all debug data. Call this from the app on tick or manual refresh.
    pub fn refresh(&mut self) {
        self.sessions = registry::all();
        self.containers = gather_containers();
        self.last_refresh = Some(Instant::now());
    }

    pub fn needs_refresh(&self) -> bool {
        match self.last_refresh {
            None => true,
            Some(t) => t.elapsed().as_secs() >= 2,
        }
    }
}

impl Component for DebugView {
    fn update(&mut self, action: &Action) {
        match action {
            Action::ScrollUp => self.scroll = self.scroll.saturating_sub(1),
            Action::ScrollDown => self.scroll = self.scroll.saturating_add(1),
            _ => {}
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect) {
        let p = Palette::default();
        let block = panel_block_accent("Debug", &p);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let mut lines: Vec<Line> = Vec::new();

        // Header
        let refresh_ago = self
            .last_refresh
            .map(|t| format!("{}s ago", t.elapsed().as_secs()))
            .unwrap_or_else(|| "never".to_string());
        lines.push(Line::from(Span::styled(
            format!("  Last refresh: {refresh_ago}"),
            Style::default().fg(p.text_tertiary),
        )));
        lines.push(Line::from(""));

        // SSH Sessions
        lines.push(Line::from(Span::styled(
            "  SSH SESSIONS",
            Style::default()
                .fg(p.text_bright)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));

        if self.sessions.is_empty() {
            lines.push(Line::from(Span::styled(
                "    No active sessions",
                Style::default().fg(p.text_disabled),
            )));
        } else {
            for s in &self.sessions {
                let (icon, icon_color) = if s.connected {
                    ("●", p.green_primary)
                } else {
                    ("✗", p.red_error)
                };

                let label = if s.label.is_empty() {
                    String::new()
                } else {
                    format!("  [{}]", s.label)
                };

                let uptime = format_duration(s.uptime);

                lines.push(Line::from(vec![
                    Span::styled(format!("    {icon} "), Style::default().fg(icon_color)),
                    Span::styled(
                        s.destination.clone(),
                        Style::default()
                            .fg(p.text_default)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(label, Style::default().fg(p.blue_info)),
                    Span::styled(
                        format!("  {uptime}  cmds: {}", s.command_count),
                        Style::default().fg(p.text_secondary),
                    ),
                ]));

                // Last command
                if let Some(ref cmd) = s.last_command {
                    let ago = s
                        .last_command_ago
                        .map(|d| format!("{}s ago", d.as_secs()))
                        .unwrap_or_default();
                    lines.push(Line::from(Span::styled(
                        format!("      last: {cmd} ({ago})"),
                        Style::default().fg(p.text_tertiary),
                    )));
                }

                // Error (only shown when disconnected)
                if let Some(ref error) = s.last_error {
                    lines.push(Line::from(Span::styled(
                        format!("      error: {error}"),
                        Style::default().fg(p.yellow_warn),
                    )));
                }

                lines.push(Line::from(""));
            }
        }
        lines.push(Line::from(""));

        // Docker Containers
        lines.push(Line::from(Span::styled(
            "  DOCKER CONTAINERS",
            Style::default()
                .fg(p.text_bright)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));

        if self.containers.is_empty() {
            lines.push(Line::from(Span::styled(
                "    No containers found",
                Style::default().fg(p.text_disabled),
            )));
        } else {
            // Header row
            lines.push(Line::from(vec![
                Span::styled(
                    format!("    {:<24}", "NAME"),
                    Style::default().fg(p.text_tertiary),
                ),
                Span::styled(
                    format!("{:<20}", "STATUS"),
                    Style::default().fg(p.text_tertiary),
                ),
                Span::styled("PORTS", Style::default().fg(p.text_tertiary)),
            ]));
            for c in &self.containers {
                let status_color = if c.status.contains("Up") {
                    p.green_primary
                } else {
                    p.yellow_warn
                };
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("    {:<24}", c.name),
                        Style::default().fg(p.text_default),
                    ),
                    Span::styled(
                        format!("{:<20}", c.status),
                        Style::default().fg(status_color),
                    ),
                    Span::styled(c.ports.clone(), Style::default().fg(p.text_tertiary)),
                ]));
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

/// Gather Docker container info.
fn gather_containers() -> Vec<ContainerInfo> {
    let output = std::process::Command::new("docker")
        .args([
            "ps",
            "--format",
            "{{.Names}}\t{{.Status}}\t{{.Ports}}",
            "--filter",
            "name=whoah",
        ])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return Vec::new(),
    };

    output
        .lines()
        .filter(|l| !l.is_empty())
        .map(|line| {
            let parts: Vec<&str> = line.splitn(3, '\t').collect();
            ContainerInfo {
                name: parts.first().unwrap_or(&"").to_string(),
                status: parts.get(1).unwrap_or(&"").to_string(),
                ports: parts.get(2).unwrap_or(&"").to_string(),
            }
        })
        .collect()
}
