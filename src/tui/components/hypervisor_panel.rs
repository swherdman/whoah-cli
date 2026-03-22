use std::cell::Cell;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::config_detail::{
    self, ConfigPanel, DetailLine, DetailStyle, LineAction, PanelAction, PanelData, PanelEvent,
    push_danger_action, push_editable, push_field, push_header, render_detail_lines,
};
use super::popup_picker::{PopupAction, PopupPicker};
use crate::config::editor::{delete_hypervisor, update_hypervisor_field};
use crate::config::loader::load_hypervisor;
use crate::config::types::{HypervisorConfig, HypervisorType};
use crate::tui::theme::Palette;

enum EditMode {
    Viewing,
    Editing { input: Input },
    TypePicker { picker: PopupPicker },
    DeleteConfirm,
}

pub struct HypervisorPanel {
    name: String,
    config: HypervisorConfig,
    referencing_deployments: Vec<String>,

    lines: Vec<DetailLine>,
    selected: usize,
    scroll_offset: usize,
    edit_mode: EditMode,
    visible_height: Cell<usize>,
}

impl HypervisorPanel {
    pub fn new(
        name: String,
        config: HypervisorConfig,
        referencing_deployments: Vec<String>,
    ) -> Self {
        let mut panel = Self {
            name,
            config,
            referencing_deployments,
            lines: Vec::new(),
            selected: 0,
            scroll_offset: 0,
            edit_mode: EditMode::Viewing,
            visible_height: Cell::new(0),
        };
        panel.rebuild_lines();
        panel.selected = config_detail::first_editable_line(&panel.lines);
        panel
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    // --- Navigation ---

    fn navigate_up(&mut self) {
        if let Some(prev) = config_detail::prev_editable_line(&self.lines, self.selected) {
            self.selected = prev;
            self.ensure_visible();
        }
    }

    fn navigate_down(&mut self) {
        if let Some(next) = config_detail::next_editable_line(&self.lines, self.selected) {
            self.selected = next;
            self.ensure_visible();
        }
    }

    fn ensure_visible(&mut self) {
        config_detail::ensure_visible(
            self.selected,
            &mut self.scroll_offset,
            self.visible_height.get(),
        );
    }

    // --- Editing ---

    fn start_edit(&mut self) -> PanelEvent {
        let Some(line) = self.lines.get(self.selected) else {
            return PanelEvent::Consumed;
        };

        // Handle action lines (delete)
        if let Some(action) = &line.action {
            return match action {
                LineAction::DeleteHypervisor => self.start_delete(),
            };
        }

        // Handle editable fields
        if line.field.is_none() {
            return PanelEvent::Consumed;
        }

        // Check if this is the type field (uses picker)
        let is_type_field = line
            .field
            .as_ref()
            .map(|f| f.path == "hypervisor.type")
            .unwrap_or(false);

        if is_type_field {
            let options = vec!["proxmox".to_string()]; // Only proxmox for now
            self.edit_mode = EditMode::TypePicker {
                picker: PopupPicker::new("Select hypervisor type", options),
            };
            return PanelEvent::Consumed;
        }

        let value = line.raw_value.clone().unwrap_or_default();
        self.edit_mode = EditMode::Editing {
            input: Input::new(value),
        };
        PanelEvent::Consumed
    }

    fn confirm_edit(&mut self) -> Result<(), String> {
        let EditMode::Editing { ref input } = self.edit_mode else {
            return Ok(());
        };
        let new_value = input.value().to_string();
        self.edit_mode = EditMode::Viewing;
        self.persist_field(&new_value)
    }

    fn persist_field(&mut self, new_value: &str) -> Result<(), String> {
        let Some(line) = self.lines.get(self.selected) else {
            return Ok(());
        };
        let Some(field_key) = &line.field else {
            return Ok(());
        };
        let path = field_key.path.clone();

        if let Err(e) = update_hypervisor_field(&self.name, &path, new_value) {
            let msg = format!("Failed to save {path}: {e}");
            tracing::error!("{msg}");
            self.reload();
            return Err(msg);
        }

        self.reload();
        Ok(())
    }

    fn reload(&mut self) {
        if let Ok(config) = load_hypervisor(&self.name) {
            self.config = config;
            self.rebuild_lines();
        }
    }

    fn start_delete(&mut self) -> PanelEvent {
        if !self.referencing_deployments.is_empty() {
            // Can't delete — referenced
            return PanelEvent::Consumed;
        }
        self.edit_mode = EditMode::DeleteConfirm;
        PanelEvent::Consumed
    }

    fn confirm_delete(&mut self) -> PanelEvent {
        self.edit_mode = EditMode::Viewing;
        if let Err(e) = delete_hypervisor(&self.name) {
            return PanelEvent::Action(PanelAction::Error(format!(
                "Failed to delete: {e}"
            )));
        }
        PanelEvent::Action(PanelAction::Deleted {
            name: self.name.clone(),
        })
    }

    fn handle_type_picker_key(&mut self, key: KeyEvent) -> PanelEvent {
        let EditMode::TypePicker { ref mut picker } = self.edit_mode else {
            return PanelEvent::Consumed;
        };
        match picker.handle_key(key) {
            PopupAction::Continue => PanelEvent::Consumed,
            PopupAction::Cancel => {
                self.edit_mode = EditMode::Viewing;
                PanelEvent::Consumed
            }
            PopupAction::Selected(_, value) => {
                self.edit_mode = EditMode::Viewing;
                // Persist the type change
                if let Err(msg) = self.persist_field(&value) {
                    PanelEvent::Action(PanelAction::Error(msg))
                } else {
                    PanelEvent::Consumed
                }
            }
        }
    }

    // --- Detail line building ---

    fn rebuild_lines(&mut self) {
        let mut lines = Vec::new();

        // GENERAL section
        push_header(&mut lines, "GENERAL");
        push_field(&mut lines, "name", &self.config.hypervisor.name);
        push_editable(
            &mut lines,
            "type",
            &format!("{:?}", self.config.hypervisor.hypervisor_type).to_lowercase(),
            "hypervisor",
            "hypervisor.type",
        );

        // CREDENTIALS section
        push_header(&mut lines, "CREDENTIALS");
        push_editable(
            &mut lines,
            "host",
            &self.config.credentials.host,
            "hypervisor",
            "credentials.host",
        );
        push_editable(
            &mut lines,
            "ssh_user",
            &self.config.credentials.ssh_user,
            "hypervisor",
            "credentials.ssh_user",
        );

        // Type-specific section
        build_type_section(&mut lines, &self.config);

        // Delete action
        if self.referencing_deployments.is_empty() {
            push_danger_action(
                &mut lines,
                "Delete hypervisor",
                LineAction::DeleteHypervisor,
            );
        } else {
            let refs = self.referencing_deployments.join(", ");
            // Disabled delete — show as regular field with explanation
            lines.push(DetailLine {
                text: String::new(),
                style: DetailStyle::SectionHeader,
                field: None,
                raw_value: None,
                picker: None,
                action: None,
            });
            lines.push(DetailLine {
                text: format!("  Delete hypervisor (referenced by {refs})"),
                style: DetailStyle::Field,
                field: None,
                raw_value: None,
                picker: None,
                action: None,
            });

            push_header(&mut lines, "REFERENCED BY");
            for dep_name in &self.referencing_deployments {
                push_field(&mut lines, " ", dep_name);
            }
        }

        self.lines = lines;
    }
}

/// Dispatch type-specific sections.
fn build_type_section(lines: &mut Vec<DetailLine>, config: &HypervisorConfig) {
    match config.hypervisor.hypervisor_type {
        HypervisorType::Proxmox => build_proxmox_section(lines, config),
        HypervisorType::LinuxKvm => {
            push_header(lines, "LINUX KVM");
            push_field(lines, "status", "(not yet supported)");
        }
    }
}

fn build_proxmox_section(lines: &mut Vec<DetailLine>, config: &HypervisorConfig) {
    push_header(lines, "PROXMOX");
    if let Some(px) = &config.proxmox {
        push_editable(lines, "node", &px.node, "hypervisor", "proxmox.node");
        push_editable(
            lines,
            "iso_storage",
            &px.iso_storage,
            "hypervisor",
            "proxmox.iso_storage",
        );
        push_editable(
            lines,
            "disk_storage",
            &px.disk_storage,
            "hypervisor",
            "proxmox.disk_storage",
        );
        push_editable(
            lines,
            "iso_file",
            &px.iso_file,
            "hypervisor",
            "proxmox.iso_file",
        );
    } else {
        push_field(lines, "error", "Missing [proxmox] section");
    }
}

impl ConfigPanel for HypervisorPanel {
    fn title(&self) -> String {
        let type_str = format!("{:?}", self.config.hypervisor.hypervisor_type).to_lowercase();
        format!("{} [{}]", self.name, type_str)
    }

    fn is_capturing(&self) -> bool {
        !matches!(self.edit_mode, EditMode::Viewing)
    }

    fn handle_key(&mut self, key: KeyEvent) -> PanelEvent {
        // Type picker overlay
        if matches!(self.edit_mode, EditMode::TypePicker { .. }) {
            return self.handle_type_picker_key(key);
        }

        // Inline text editing
        if let EditMode::Editing { .. } = self.edit_mode {
            match key.code {
                KeyCode::Enter => {
                    if let Err(msg) = self.confirm_edit() {
                        return PanelEvent::Action(PanelAction::Error(msg));
                    }
                }
                KeyCode::Esc => self.edit_mode = EditMode::Viewing,
                _ => {
                    if let EditMode::Editing { ref mut input } = self.edit_mode {
                        input.handle_event(&crossterm::event::Event::Key(key));
                    }
                }
            }
            return PanelEvent::Consumed;
        }

        // Delete confirmation
        if matches!(self.edit_mode, EditMode::DeleteConfirm) {
            match key.code {
                KeyCode::Enter => return self.confirm_delete(),
                KeyCode::Esc => self.edit_mode = EditMode::Viewing,
                _ => {}
            }
            return PanelEvent::Consumed;
        }

        // Normal panel navigation
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.navigate_down();
                PanelEvent::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.navigate_up();
                PanelEvent::Consumed
            }
            KeyCode::Enter => self.start_edit(),
            KeyCode::Char('h') | KeyCode::Left => PanelEvent::Ignored, // No tabs
            KeyCode::Char('l') | KeyCode::Right => PanelEvent::Ignored,
            KeyCode::Esc => PanelEvent::Ignored,
            _ => PanelEvent::Ignored,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, palette: &Palette) {
        let p = palette;

        if self.lines.is_empty() {
            frame.render_widget(
                Paragraph::new("  No configuration loaded.")
                    .style(Style::default().fg(p.text_tertiary).bg(p.bg_panel)),
                area,
            );
            return;
        }

        // Handle delete confirm: override the delete line text
        let delete_confirm = matches!(self.edit_mode, EditMode::DeleteConfirm);

        if delete_confirm {
            // Render lines with the delete line replaced by confirmation text
            let mut modified_lines: Vec<DetailLine> = Vec::new();
            for (i, line) in self.lines.iter().enumerate() {
                if line.action == Some(LineAction::DeleteHypervisor) {
                    modified_lines.push(DetailLine {
                        text: format!("  Delete {}? Enter to confirm, Esc to cancel", self.name),
                        style: DetailStyle::DangerAction,
                        field: None,
                        raw_value: None,
                        picker: None,
                        action: Some(LineAction::DeleteHypervisor),
                    });
                } else {
                    // Can't clone DetailLine easily, so re-create with refs
                    modified_lines.push(DetailLine {
                        text: line.text.clone(),
                        style: line.style,
                        field: line.field.clone(),
                        raw_value: line.raw_value.clone(),
                        picker: line.picker.clone(),
                        action: line.action.clone(),
                    });
                }
            }

            let vis_h = render_detail_lines(
                frame,
                area,
                &modified_lines,
                self.selected,
                self.scroll_offset,
                true,
                None,
                p,
            );
            self.visible_height.set(vis_h);
        } else {
            let edit_input = if let EditMode::Editing { ref input } = self.edit_mode {
                Some(input)
            } else {
                None
            };

            let vis_h = render_detail_lines(
                frame,
                area,
                &self.lines,
                self.selected,
                self.scroll_offset,
                true,
                edit_input,
                p,
            );
            self.visible_height.set(vis_h);
        }

        // Overlay: type picker
        if let EditMode::TypePicker { ref picker } = self.edit_mode {
            picker.render(frame, area, p);
        }
    }
}
