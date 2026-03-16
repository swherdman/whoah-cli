use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::action::Screen;
use crate::tui::theme::Palette;

use super::Component;

pub struct StatusBarComponent {
    deployment_name: String,
    pub(crate) host: String,
    pub(crate) connected: bool,
}

impl StatusBarComponent {
    pub fn new(deployment_name: &str) -> Self {
        Self {
            deployment_name: deployment_name.to_string(),
            host: String::new(),
            connected: false,
        }
    }

    pub fn set_connected(&mut self, host: &str) {
        self.host = host.to_string();
        self.connected = true;
    }

    pub fn render_tab_bar(&self, frame: &mut Frame, area: Rect, active: Screen) {
        let p = Palette::default();

        let tabs = [
            (Screen::Build, "Build", "1"),
            (Screen::Config, "Config", "2"),
            (Screen::Monitor, "Monitor", "3"),
            (Screen::Debug, "Debug", "d"),
        ];

        let mut spans: Vec<Span> = vec![Span::raw(" ")];

        for (screen, label, key) in &tabs {
            if *screen == active {
                spans.push(Span::styled(
                    format!("[{label}]"),
                    Style::default()
                        .fg(p.green_primary)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(
                    format!(" {key}:"),
                    Style::default().fg(p.text_disabled),
                ));
                spans.push(Span::styled(
                    label.to_string(),
                    Style::default().fg(p.text_tertiary),
                ));
            }
            spans.push(Span::raw("  "));
        }

        // Right-align deployment name + connection status
        let right_info = if self.connected {
            format!("{}  {} online ", self.deployment_name, self.host)
        } else {
            format!("{}  connecting... ", self.deployment_name)
        };

        // Calculate space needed
        let left_len: usize = spans.iter().map(|s| s.width()).sum();
        let right_len = right_info.len();
        let padding = (area.width as usize).saturating_sub(left_len + right_len);
        spans.push(Span::raw(" ".repeat(padding)));

        if self.connected {
            spans.push(Span::styled(
                format!("{} ", self.deployment_name),
                Style::default().fg(p.text_bright).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                format!("{}  ", self.host),
                Style::default().fg(p.text_default),
            ));
            spans.push(Span::styled("●", Style::default().fg(p.green_primary)));
            spans.push(Span::raw(" "));
        } else {
            spans.push(Span::styled(
                format!("{} ", self.deployment_name),
                Style::default().fg(p.text_bright).add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled("connecting... ", Style::default().fg(p.yellow_warn)));
        }

        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::default().bg(p.bg_hover)),
            area,
        );
    }

    pub fn render_keybindings_for_screen(&self, frame: &mut Frame, area: Rect, screen: Screen) {
        let p = Palette::default();

        let mut spans = vec![
            Span::styled(" 1-3", Style::default().fg(p.green_primary)),
            Span::styled(":tabs  ", Style::default().fg(p.text_secondary)),
        ];

        match screen {
            Screen::Monitor => {
                spans.extend([
                    Span::styled("[r]", Style::default().fg(p.green_primary)),
                    Span::styled("ecover ", Style::default().fg(p.text_secondary)),
                    Span::styled("[s]", Style::default().fg(p.green_primary)),
                    Span::styled("tatus ", Style::default().fg(p.text_secondary)),
                    Span::styled("Tab", Style::default().fg(p.green_primary)),
                    Span::styled(":panels ", Style::default().fg(p.text_secondary)),
                    Span::styled("j/k", Style::default().fg(p.green_primary)),
                    Span::styled(":scroll  ", Style::default().fg(p.text_secondary)),
                ]);
            }
            Screen::Build => {
                spans.extend([
                    Span::styled("[b]", Style::default().fg(p.green_primary)),
                    Span::styled("uild  ", Style::default().fg(p.text_secondary)),
                    Span::styled("j/k", Style::default().fg(p.green_primary)),
                    Span::styled(":scroll  ", Style::default().fg(p.text_secondary)),
                ]);
            }
            Screen::Config => {}
            Screen::Debug => {
                spans.extend([
                    Span::styled("[r]", Style::default().fg(p.green_primary)),
                    Span::styled("efresh  ", Style::default().fg(p.text_secondary)),
                    Span::styled("j/k", Style::default().fg(p.green_primary)),
                    Span::styled(":scroll  ", Style::default().fg(p.text_secondary)),
                ]);
            }
        }

        spans.extend([
            Span::styled("[q]", Style::default().fg(p.green_primary)),
            Span::styled("uit", Style::default().fg(p.text_secondary)),
        ]);

        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::default().bg(p.bg_hover)),
            area,
        );
    }
}

impl Component for StatusBarComponent {
    fn render(&self, frame: &mut Frame, area: Rect) {
        self.render_keybindings_for_screen(frame, area, Screen::Monitor);
    }
}
