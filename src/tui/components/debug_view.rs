use std::time::Instant;

use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::ssh::registry::{self, SessionSnapshot};
use crate::tui::theme::{panel_block_accent, Palette};

use super::Component;

pub struct DebugView {
    sessions: Vec<SessionSnapshot>,
    mux_masters: Vec<MuxMasterInfo>,
    containers: Vec<ContainerInfo>,
    scroll: u16,
    last_refresh: Option<Instant>,
}

pub struct MuxMasterInfo {
    pub pid: String,
    pub destination: String,
    pub control_persist: String,
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
            mux_masters: Vec::new(),
            containers: Vec::new(),
            scroll: 0,
            last_refresh: None,
        }
    }

    pub fn scroll_up(&mut self) {
        self.scroll = self.scroll.saturating_sub(1);
    }

    pub fn scroll_down(&mut self) {
        self.scroll = self.scroll.saturating_add(1);
    }

    /// Refresh all debug data. Call this from the app on tick or manual refresh.
    pub fn refresh(&mut self) {
        self.sessions = registry::all();
        self.mux_masters = gather_mux_masters();
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
                let (icon, icon_color) = if s.socket_exists {
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

                // Control socket path + status
                let socket_status = if s.socket_exists {
                    Span::styled("exists", Style::default().fg(p.green_primary))
                } else {
                    Span::styled(
                        "MISSING",
                        Style::default()
                            .fg(p.red_error)
                            .add_modifier(Modifier::BOLD),
                    )
                };

                let ctl_short = s
                    .ctl_path
                    .to_string_lossy()
                    .chars()
                    .rev()
                    .take(50)
                    .collect::<String>()
                    .chars()
                    .rev()
                    .collect::<String>();

                lines.push(Line::from(vec![
                    Span::styled(
                        format!("      ctl: ...{ctl_short}  socket: "),
                        Style::default().fg(p.text_disabled),
                    ),
                    socket_status,
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

                // Master log tail (only shown when socket is missing)
                if let Some(ref log) = s.master_log_tail {
                    for log_line in log.lines() {
                        lines.push(Line::from(Span::styled(
                            format!("      log: {log_line}"),
                            Style::default().fg(p.yellow_warn),
                        )));
                    }
                }

                lines.push(Line::from(""));
            }
        }

        // Mux Masters
        lines.push(Line::from(Span::styled(
            "  MUX MASTERS (system-wide)",
            Style::default()
                .fg(p.text_bright)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));

        if self.mux_masters.is_empty() {
            lines.push(Line::from(Span::styled(
                "    No mux masters found",
                Style::default().fg(p.text_disabled),
            )));
        } else {
            for m in &self.mux_masters {
                lines.push(Line::from(Span::styled(
                    format!("    PID {}  {}  {}", m.pid, m.destination, m.control_persist),
                    Style::default().fg(p.text_secondary),
                )));
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
            for c in &self.containers {
                let status_color = if c.status.contains("Up") {
                    p.green_primary
                } else {
                    p.yellow_warn
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("    {:<24}", c.name), Style::default().fg(p.text_default)),
                    Span::styled(format!("{:<20}", c.status), Style::default().fg(status_color)),
                    Span::styled(c.ports.clone(), Style::default().fg(p.text_tertiary)),
                ]));
            }
        }

        // Apply scroll
        let height = inner.height as usize;
        let max_scroll = lines.len().saturating_sub(height);
        let scroll = (self.scroll as usize).min(max_scroll);
        let visible: Vec<Line> = lines.into_iter().skip(scroll).take(height).collect();

        frame.render_widget(Paragraph::new(visible), inner);
    }
}

/// Gather SSH mux master processes from the system.
fn gather_mux_masters() -> Vec<MuxMasterInfo> {
    let output = std::process::Command::new("sh")
        .args(["-c", "ps aux | grep 'ssh.*-M.*-f.*-N' | grep -v grep"])
        .output();

    let output = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).to_string(),
        _ => return Vec::new(),
    };

    output
        .lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 11 {
                return None;
            }
            let pid = fields[1].to_string();
            // Find the destination (last non-flag arg)
            let destination = fields
                .iter()
                .rev()
                .find(|f| !f.starts_with('-') && f.contains('@'))
                .unwrap_or(&"unknown")
                .to_string();
            // Find ControlPersist value
            let control_persist = fields
                .iter()
                .find(|f| f.starts_with("ControlPersist"))
                .unwrap_or(&"unknown")
                .to_string();

            Some(MuxMasterInfo {
                pid,
                destination,
                control_persist,
            })
        })
        .collect()
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

fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs >= 3600 {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}s", secs)
    }
}
