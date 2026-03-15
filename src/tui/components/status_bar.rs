use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

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

    /// Render the title bar at the top of the screen (host info + connection status).
    pub fn render_title_bar(&self, frame: &mut Frame, area: Rect) {
        let conn_status = if self.connected {
            vec![
                Span::styled(
                    format!(" whoah — {} ", self.deployment_name),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{} ", self.host),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    "online ",
                    Style::default().fg(Color::Green),
                ),
            ]
        } else {
            vec![
                Span::styled(
                    format!(" whoah — {} ", self.deployment_name),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    "connecting... ",
                    Style::default().fg(Color::Yellow),
                ),
            ]
        };

        frame.render_widget(
            Paragraph::new(Line::from(conn_status)).style(Style::default().bg(Color::DarkGray)),
            area,
        );
    }

    /// Render the keybindings bar at the bottom of the screen.
    pub fn render_keybindings(&self, frame: &mut Frame, area: Rect) {
        let line = Line::from(vec![
            Span::styled(" [r]", Style::default().fg(Color::Cyan)),
            Span::raw("ecover "),
            Span::styled("[s]", Style::default().fg(Color::Cyan)),
            Span::raw("tatus "),
            Span::styled("[q]", Style::default().fg(Color::Cyan)),
            Span::raw("uit  "),
            Span::styled("Tab", Style::default().fg(Color::Cyan)),
            Span::raw(":panels "),
            Span::styled("j/k", Style::default().fg(Color::Cyan)),
            Span::raw(":scroll"),
        ]);

        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(Color::DarkGray)),
            area,
        );
    }
}

impl Component for StatusBarComponent {
    fn render(&self, frame: &mut Frame, area: Rect) {
        // Default render used in recovery mode — just keybindings
        self.render_keybindings(frame, area);
    }
}
