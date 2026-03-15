use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::event::Severity;

use super::Component;

pub struct AlertBar {
    pub message: Option<(Severity, String)>,
}

impl AlertBar {
    pub fn new() -> Self {
        Self { message: None }
    }

    pub fn set_alert(&mut self, severity: Severity, message: String) {
        self.message = Some((severity, message));
    }

    pub fn clear(&mut self) {
        self.message = None;
    }
}

impl Component for AlertBar {
    fn render(&self, frame: &mut Frame, area: Rect) {
        let (style, text) = match &self.message {
            Some((Severity::Critical, msg)) => (
                Style::default().fg(Color::White).bg(Color::Red).add_modifier(Modifier::BOLD),
                format!(" !! {msg}"),
            ),
            Some((Severity::Warning, msg)) => (
                Style::default().fg(Color::Black).bg(Color::Yellow),
                format!(" ! {msg}"),
            ),
            Some((Severity::Info, msg)) => (
                Style::default().fg(Color::White),
                format!("   {msg}"),
            ),
            None => return,
        };

        frame.render_widget(Paragraph::new(text).style(style), area);
    }
}
