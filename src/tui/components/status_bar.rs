use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::tui::theme::Palette;

use super::Component;

pub struct StatusBarComponent {
    deployment_name: String,
    host: String,
    connected: bool,
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

    pub fn render_title_bar(&self, frame: &mut Frame, area: Rect) {
        let p = Palette::default();

        let spans = if self.connected {
            vec![
                Span::styled(
                    format!(" whoah — {} ", self.deployment_name),
                    Style::default().fg(p.text_bright).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{} ", self.host),
                    Style::default().fg(p.text_default),
                ),
                Span::styled("online ", Style::default().fg(p.green_primary)),
            ]
        } else {
            vec![
                Span::styled(
                    format!(" whoah — {} ", self.deployment_name),
                    Style::default().fg(p.text_bright).add_modifier(Modifier::BOLD),
                ),
                Span::styled("connecting... ", Style::default().fg(p.yellow_warn)),
            ]
        };

        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::default().bg(p.bg_hover)),
            area,
        );
    }

    pub fn render_keybindings(&self, frame: &mut Frame, area: Rect) {
        let p = Palette::default();

        let line = Line::from(vec![
            Span::styled(" [r]", Style::default().fg(p.green_primary)),
            Span::styled("ecover ", Style::default().fg(p.text_secondary)),
            Span::styled("[s]", Style::default().fg(p.green_primary)),
            Span::styled("tatus ", Style::default().fg(p.text_secondary)),
            Span::styled("[q]", Style::default().fg(p.green_primary)),
            Span::styled("uit  ", Style::default().fg(p.text_secondary)),
            Span::styled("Tab", Style::default().fg(p.green_primary)),
            Span::styled(":panels ", Style::default().fg(p.text_secondary)),
            Span::styled("j/k", Style::default().fg(p.green_primary)),
            Span::styled(":scroll", Style::default().fg(p.text_secondary)),
        ]);

        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(p.bg_hover)),
            area,
        );
    }
}

impl Component for StatusBarComponent {
    fn render(&self, frame: &mut Frame, area: Rect) {
        self.render_keybindings(frame, area);
    }
}
