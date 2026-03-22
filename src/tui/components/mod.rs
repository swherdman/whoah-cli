pub mod alert_bar;
pub mod build_view;
pub mod config_detail;
pub mod config_view;
pub mod deployment_panel;
pub mod debug_view;
pub mod git_ref_selector;
pub mod hypervisor_panel;
pub mod popup_picker;
pub mod disk_panel;
pub mod log_panel;
pub mod recovery_view;
pub mod status_bar;
pub mod status_panel;

use ratatui::prelude::*;

use crate::action::Action;

pub trait Component {
    /// Update state based on an action dispatched by the app.
    fn update(&mut self, _action: &Action) {}

    /// Render into the given area. Must be pure (no state mutation).
    fn render(&self, frame: &mut Frame, area: Rect);
}
