use ratatui::prelude::*;
use ratatui::widgets::Block;
use tui_logger::{TuiLoggerWidget, TuiWidgetState};

use crate::tui::theme::Palette;

use super::Component;

pub struct LogPanel {
    state: TuiWidgetState,
}

impl LogPanel {
    pub fn new() -> Self {
        Self {
            state: TuiWidgetState::new().set_default_display_level(log::LevelFilter::Info),
        }
    }
}

impl Component for LogPanel {
    fn render(&self, frame: &mut Frame, area: Rect) {
        let p = Palette::default();

        let widget = TuiLoggerWidget::default()
            .block(
                Block::bordered()
                    .title(" LOGS ")
                    .border_style(Style::default().fg(p.border_default))
                    .title_style(Style::default().fg(p.text_tertiary))
                    .style(Style::default().bg(p.bg_panel))
                    .padding(ratatui::widgets::Padding::new(1, 1, 0, 0)),
            )
            .style(Style::default().fg(p.text_tertiary))
            .style_error(Style::default().fg(p.red_error))
            .style_warn(Style::default().fg(p.yellow_warn))
            .style_info(Style::default().fg(p.text_default))
            .style_debug(Style::default().fg(p.text_disabled))
            .style_trace(Style::default().fg(p.text_disabled))
            .output_target(false)
            .output_file(false)
            .output_line(false)
            .state(&self.state);

        frame.render_widget(widget, area);
    }
}
