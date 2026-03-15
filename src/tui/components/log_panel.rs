use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::tui::theme::{self, Palette};

use super::Component;

pub struct LogPanel {
    messages: Vec<String>,
    max_messages: usize,
}

impl LogPanel {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            max_messages: 200,
        }
    }

    pub fn push(&mut self, message: String) {
        self.messages.push(message);
        if self.messages.len() > self.max_messages {
            self.messages.remove(0);
        }
    }
}

impl Component for LogPanel {
    fn render(&self, frame: &mut Frame, area: Rect) {
        let p = Palette::default();
        let block = theme::panel_block("Logs", false, &p);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let height = inner.height as usize;
        let start = self.messages.len().saturating_sub(height);
        let visible: Vec<Line> = self.messages[start..]
            .iter()
            .map(|m| Line::from(Span::styled(m.as_str(), Style::default().fg(p.text_tertiary))))
            .collect();

        frame.render_widget(Paragraph::new(visible), inner);
    }
}
