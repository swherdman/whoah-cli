pub mod alert_bar;
pub mod disk_panel;
pub mod log_panel;
pub mod recovery_view;
pub mod status_bar;
pub mod status_panel;

use ratatui::prelude::*;

use crate::action::Action;
use crate::event::Event;

pub trait Component {
    /// Handle an event, optionally return an action.
    fn handle_event(&mut self, _event: &Event) -> Option<Action> {
        None
    }

    /// Update state based on an action.
    fn update(&mut self, _action: &Action) {}

    /// Render into the given area.
    fn render(&self, frame: &mut Frame, area: Rect);
}
