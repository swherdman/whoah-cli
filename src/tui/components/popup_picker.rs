use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};

use crate::tui::theme::Palette;

/// Action returned from PopupPicker::handle_key.
pub enum PopupAction {
    /// Keep the picker open.
    Continue,
    /// User selected an option (index + value).
    Selected(usize, String),
    /// User cancelled.
    Cancel,
}

/// A reusable popup for selecting from a list of options.
/// Renders as a small centered bordered box overlaying the content area.
pub struct PopupPicker {
    title: String,
    options: Vec<String>,
    selected: usize,
}

impl PopupPicker {
    pub fn new(title: impl Into<String>, options: Vec<String>) -> Self {
        Self {
            title: title.into(),
            options,
            selected: 0,
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) -> PopupAction {
        match key.code {
            KeyCode::Esc => PopupAction::Cancel,
            KeyCode::Enter => {
                if let Some(value) = self.options.get(self.selected) {
                    PopupAction::Selected(self.selected, value.clone())
                } else {
                    PopupAction::Cancel
                }
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if !self.options.is_empty() {
                    self.selected = (self.selected + 1).min(self.options.len() - 1);
                }
                PopupAction::Continue
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.selected = self.selected.saturating_sub(1);
                PopupAction::Continue
            }
            _ => PopupAction::Continue,
        }
    }

    pub fn render(&self, frame: &mut Frame, area: Rect, palette: &Palette) {
        let p = palette;

        let item_count = self.options.len() as u16;
        let popup_h = (item_count + 2).min(area.height.saturating_sub(2)).max(3); // +2 for border
        let popup_w = {
            let max_option_len = self.options.iter().map(|s| s.len()).max().unwrap_or(10);
            let title_len = self.title.len() + 4; // + border + padding
            (max_option_len + 6) // + padding + highlight symbol
                .max(title_len)
                .min(area.width.saturating_sub(4) as usize) as u16
        };

        let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
        let y = area.y + (area.height.saturating_sub(popup_h)) / 2;
        let popup_area = Rect::new(x, y, popup_w, popup_h);

        frame.render_widget(Clear, popup_area);

        let block = Block::default()
            .title(format!(" {} ", self.title))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(p.green_border))
            .title_style(Style::default().fg(p.text_bright));
        let inner = block.inner(popup_area);
        frame.render_widget(block, popup_area);

        if inner.height == 0 {
            return;
        }

        if self.options.is_empty() {
            frame.render_widget(
                Paragraph::new("  (no options)")
                    .style(Style::default().fg(p.text_tertiary)),
                inner,
            );
            return;
        }

        let items: Vec<ListItem> = self
            .options
            .iter()
            .map(|s| ListItem::new(format!(" {s}")).style(Style::default().fg(p.text_default)))
            .collect();
        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .fg(p.green_primary)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(">");
        let mut state = ListState::default();
        state.select(Some(self.selected));
        frame.render_stateful_widget(list, inner, &mut state);
    }
}
