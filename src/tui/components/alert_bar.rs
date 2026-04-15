use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::event::Severity;
use crate::tui::theme::Palette;

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
        let p = Palette::default();

        let (style, text) = match &self.message {
            Some((Severity::Critical, msg)) => (
                Style::default()
                    .fg(p.text_bright)
                    .bg(p.red_error)
                    .add_modifier(Modifier::BOLD),
                format!(" !! {msg}"),
            ),
            Some((Severity::Warning, msg)) => (
                Style::default().fg(p.text_bright).bg(p.yellow_warn),
                format!(" ! {msg}"),
            ),
            Some((Severity::Info, msg)) => {
                (Style::default().fg(p.text_default), format!("   {msg}"))
            }
            None => return,
        };

        frame.render_widget(Paragraph::new(text).style(style), area);
    }
}
