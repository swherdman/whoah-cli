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
use crate::config::types::{HypervisorConfig, HypervisorType, ProxmoxHypervisorConfig};
use crate::ops::hypervisor_proxmox_validate::{FieldStatus, ProxmoxValidation};
use crate::ssh::probe::SshProbeStatus;
use crate::tui::theme::Palette;

enum EditMode {
    Viewing,
    Editing {
        input: Input,
    },
    /// Popup picker for selecting from a list (type, node, storage, iso_file).
    PickerSelect {
        picker: PopupPicker,
    },
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

    /// SSH credential probe status.
    ssh_status: SshProbeStatus,
    /// Proxmox config validation results (if type=proxmox).
    proxmox_validation: Option<ProxmoxValidation>,
    /// ISO download state.
    download_state: DownloadState,
}

#[derive(Clone)]
enum DownloadState {
    Idle,
    Downloading { percent: f32 },
    Failed(String),
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
            ssh_status: SshProbeStatus::Unknown,
            proxmox_validation: None,
            download_state: DownloadState::Idle,
        };
        panel.rebuild_lines();
        panel.selected = config_detail::first_editable_line(&panel.lines);
        panel
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn proxmox_config(&self) -> Option<ProxmoxHypervisorConfig> {
        self.config.proxmox.clone()
    }

    pub fn credentials_host(&self) -> &str {
        &self.config.credentials.host
    }

    pub fn credentials_user(&self) -> &str {
        &self.config.credentials.ssh_user
    }

    pub fn credentials_port(&self) -> u16 {
        self.config.credentials.ssh_port()
    }

    pub fn set_proxmox_checking(&mut self) {
        self.proxmox_validation = Some(ProxmoxValidation::checking());
        self.rebuild_lines();
    }

    /// Request an SSH credential probe if the host is non-empty.
    /// Sets status to Checking and returns the PanelAction for the caller to dispatch.
    pub fn request_probe(&mut self) -> Option<PanelAction> {
        if self.config.credentials.host.is_empty() {
            self.ssh_status = SshProbeStatus::Unknown;
            self.rebuild_lines();
            return None;
        }
        let host = self.config.credentials.host.clone();
        let user = self.config.credentials.ssh_user.clone();
        let port = self.config.credentials.ssh_port();
        self.ssh_status = SshProbeStatus::Checking;
        self.rebuild_lines();
        Some(PanelAction::ProbeSsh { host, user, port })
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

        // Handle action lines (delete, download)
        if let Some(action) = &line.action {
            return match action {
                LineAction::DeleteHypervisor => self.start_delete(),
                LineAction::DownloadIso => self.start_download(),
            };
        }

        // Handle editable fields
        if line.field.is_none() {
            return PanelEvent::Consumed;
        }

        let field_path = line.field.as_ref().map(|f| f.path.as_str()).unwrap_or("");

        // Fields that use pickers instead of free-text input
        let picker = match field_path {
            "hypervisor.type" => Some(PopupPicker::new(
                "Select hypervisor type",
                vec!["proxmox".to_string()],
            )),
            "proxmox.node" => {
                let options = self
                    .proxmox_validation
                    .as_ref()
                    .map(|v| v.available_nodes.clone())
                    .unwrap_or_default();
                if options.is_empty() {
                    None
                } else {
                    Some(PopupPicker::new("Select node", options))
                }
            }
            "proxmox.disk_storage" => {
                let options = self
                    .proxmox_validation
                    .as_ref()
                    .map(|v| v.available_disk_storages.clone())
                    .unwrap_or_default();
                if options.is_empty() {
                    None
                } else {
                    Some(PopupPicker::new("Select disk storage", options))
                }
            }
            "proxmox.iso_storage" => {
                let options = self
                    .proxmox_validation
                    .as_ref()
                    .map(|v| v.available_iso_storages.clone())
                    .unwrap_or_default();
                if options.is_empty() {
                    None
                } else {
                    Some(PopupPicker::new("Select ISO storage", options))
                }
            }
            "proxmox.iso_file" => {
                let options = self
                    .proxmox_validation
                    .as_ref()
                    .map(|v| v.available_iso_files.clone())
                    .unwrap_or_default();
                if options.is_empty() {
                    None
                } else {
                    Some(PopupPicker::new("Select ISO file", options))
                }
            }
            _ => None,
        };

        if let Some(picker) = picker {
            self.edit_mode = EditMode::PickerSelect { picker };
            return PanelEvent::Consumed;
        }

        // Default: free-text editing
        let value = line.raw_value.clone().unwrap_or_default();
        self.edit_mode = EditMode::Editing {
            input: Input::new(value),
        };
        PanelEvent::Consumed
    }

    /// Confirm the edit, persist to disk, reload.
    /// Returns Ok(Some(PanelAction)) if a re-probe should be triggered.
    fn confirm_edit(&mut self) -> Result<Option<PanelAction>, String> {
        let EditMode::Editing { ref input } = self.edit_mode else {
            return Ok(None);
        };
        let new_value = input.value().to_string();
        // Capture the field path before changing edit_mode
        let field_path = self
            .lines
            .get(self.selected)
            .and_then(|l| l.field.as_ref())
            .map(|f| f.path.clone());
        self.edit_mode = EditMode::Viewing;
        self.persist_field(&new_value)?;

        let path = field_path.as_deref().unwrap_or("");

        // Re-probe SSH if a credential field was edited
        if path == "credentials.host"
            || path == "credentials.ssh_user"
            || path == "credentials.ssh_port"
        {
            return Ok(self.request_probe());
        }

        // Re-validate Proxmox if a Proxmox field was edited via free-text
        if path.starts_with("proxmox.") && !self.config.credentials.host.is_empty() {
            self.proxmox_validation = Some(ProxmoxValidation::checking());
            self.rebuild_lines();
            return Ok(Some(PanelAction::ValidateProxmox {
                host: self.config.credentials.host.clone(),
                user: self.config.credentials.ssh_user.clone(),
                port: self.config.credentials.ssh_port(),
            }));
        }

        Ok(None)
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

    fn start_download(&mut self) -> PanelEvent {
        let storage_path = self
            .proxmox_validation
            .as_ref()
            .and_then(|v| v.iso_storage_path.clone());
        let Some(storage_path) = storage_path else {
            return PanelEvent::Action(PanelAction::Error(
                "Cannot download: iso_storage path unknown".into(),
            ));
        };
        let Some(px) = &self.config.proxmox else {
            return PanelEvent::Consumed;
        };
        let host = self.config.credentials.host.clone();
        let user = self.config.credentials.ssh_user.clone();
        let filename = px.iso_file.clone();
        self.download_state = DownloadState::Downloading { percent: 0.0 };
        self.rebuild_lines();
        PanelEvent::Action(PanelAction::DownloadIso {
            host,
            user,
            port: self.config.credentials.ssh_port(),
            iso_storage_path: storage_path,
            filename,
        })
    }

    fn start_delete(&mut self) -> PanelEvent {
        if !self.referencing_deployments.is_empty() {
            let refs = self.referencing_deployments.join(", ");
            return PanelEvent::Action(PanelAction::Error(format!(
                "Cannot delete: referenced by {refs}"
            )));
        }
        self.edit_mode = EditMode::DeleteConfirm;
        PanelEvent::Consumed
    }

    fn confirm_delete(&mut self) -> PanelEvent {
        self.edit_mode = EditMode::Viewing;
        if let Err(e) = delete_hypervisor(&self.name) {
            return PanelEvent::Action(PanelAction::Error(format!("Failed to delete: {e}")));
        }
        PanelEvent::Action(PanelAction::Deleted {
            name: self.name.clone(),
        })
    }

    fn handle_picker_key(&mut self, key: KeyEvent) -> PanelEvent {
        let EditMode::PickerSelect { ref mut picker } = self.edit_mode else {
            return PanelEvent::Consumed;
        };
        match picker.handle_key(key) {
            PopupAction::Continue => PanelEvent::Consumed,
            PopupAction::Cancel => {
                self.edit_mode = EditMode::Viewing;
                PanelEvent::Consumed
            }
            PopupAction::Selected(_, value) => {
                // Check if this is a Proxmox field before clearing edit mode
                let is_proxmox_field = self
                    .lines
                    .get(self.selected)
                    .and_then(|l| l.field.as_ref())
                    .map(|f| f.path.starts_with("proxmox."))
                    .unwrap_or(false);

                self.edit_mode = EditMode::Viewing;
                if let Err(msg) = self.persist_field(&value) {
                    return PanelEvent::Action(PanelAction::Error(msg));
                }
                // Re-validate Proxmox config after changing a Proxmox field
                if is_proxmox_field && !self.config.credentials.host.is_empty() {
                    self.proxmox_validation = Some(ProxmoxValidation::checking());
                    self.rebuild_lines();
                    PanelEvent::Action(PanelAction::ValidateProxmox {
                        host: self.config.credentials.host.clone(),
                        user: self.config.credentials.ssh_user.clone(),
                        port: self.config.credentials.ssh_port(),
                    })
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

        // CREDENTIALS section with SSH status indicator
        {
            let (dot, color) = match self.ssh_status {
                SshProbeStatus::Unknown => ("●", Palette::default().text_disabled),
                SshProbeStatus::Checking => ("●", Palette::default().text_disabled),
                SshProbeStatus::Valid => ("●", Palette::default().green_primary),
                SshProbeStatus::AuthFailed => ("●", Palette::default().red_error),
                SshProbeStatus::Offline => ("●", Palette::default().yellow_warn),
            };
            // Spacer line
            lines.push(DetailLine {
                text: String::new(),
                style: DetailStyle::SectionHeader,
                field: None,
                raw_value: None,
                picker: None,
                action: None,
                fg_override: None,
                suffix: None,
            });
            // Header with colored dot
            lines.push(DetailLine {
                text: format!("  CREDENTIALS {dot}"),
                style: DetailStyle::SectionHeader,
                field: None,
                raw_value: None,
                picker: None,
                action: None,
                fg_override: Some(color),
                suffix: None,
            });
        }
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
        {
            let port_str = self
                .config
                .credentials
                .ssh_port
                .map(|p| p.to_string())
                .unwrap_or_else(|| "22".to_string());
            push_editable(
                &mut lines,
                "ssh_port",
                &port_str,
                "hypervisor",
                "credentials.ssh_port",
            );
        }

        // Type-specific section
        build_type_section(&mut lines, &self.config, self.proxmox_validation.as_ref());

        // Download action — shown when ISO is missing or download in progress/failed
        {
            let p = Palette::default();
            let show_download = match &self.download_state {
                DownloadState::Downloading { .. } | DownloadState::Failed(_) => true,
                DownloadState::Idle => self
                    .proxmox_validation
                    .as_ref()
                    .map(|v| matches!(v.iso_file, FieldStatus::Invalid(_)))
                    .unwrap_or(false),
            };

            if show_download {
                if let Some(px) = &self.config.proxmox {
                    // Spacer
                    lines.push(DetailLine {
                        text: String::new(),
                        style: DetailStyle::SectionHeader,
                        field: None,
                        raw_value: None,
                        picker: None,
                        action: None,
                        fg_override: None,
                        suffix: None,
                    });

                    let (text, color, action) = match &self.download_state {
                        DownloadState::Idle => (
                            format!("  Download {}", px.iso_file),
                            p.green_primary,
                            Some(LineAction::DownloadIso),
                        ),
                        DownloadState::Downloading { percent } => (
                            format!("  Downloading... {percent:.1}%"),
                            p.yellow_warn,
                            None,
                        ),
                        DownloadState::Failed(msg) => (
                            format!("  Download failed: {msg}"),
                            p.red_error,
                            Some(LineAction::DownloadIso), // Allow retry
                        ),
                    };

                    lines.push(DetailLine {
                        text,
                        style: DetailStyle::EditableField,
                        field: None,
                        raw_value: None,
                        picker: None,
                        action,
                        fg_override: Some(color),
                        suffix: None,
                    });
                }
            }
        }

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
                fg_override: None,
                suffix: None,
            });
            lines.push(DetailLine {
                text: format!("  Delete hypervisor (referenced by {refs})"),
                style: DetailStyle::Field,
                field: None,
                raw_value: None,
                picker: None,
                action: None,
                fg_override: None,
                suffix: None,
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
fn build_type_section(
    lines: &mut Vec<DetailLine>,
    config: &HypervisorConfig,
    validation: Option<&ProxmoxValidation>,
) {
    match config.hypervisor.hypervisor_type {
        HypervisorType::Proxmox => build_proxmox_section(lines, config, validation),
        HypervisorType::LinuxKvm => {
            push_header(lines, "LINUX KVM");
            push_field(lines, "status", "(not yet supported)");
        }
    }
}

fn build_proxmox_section(
    lines: &mut Vec<DetailLine>,
    config: &HypervisorConfig,
    validation: Option<&ProxmoxValidation>,
) {
    push_header(lines, "PROXMOX");
    if let Some(px) = &config.proxmox {
        push_validated_field(
            lines,
            "node",
            &px.node,
            "hypervisor",
            "proxmox.node",
            validation.map(|v| &v.node),
        );
        push_validated_field(
            lines,
            "disk_storage",
            &px.disk_storage,
            "hypervisor",
            "proxmox.disk_storage",
            validation.map(|v| &v.disk_storage),
        );
        push_validated_field(
            lines,
            "iso_storage",
            &px.iso_storage,
            "hypervisor",
            "proxmox.iso_storage",
            validation.map(|v| &v.iso_storage),
        );
        push_validated_field(
            lines,
            "iso_file",
            &px.iso_file,
            "hypervisor",
            "proxmox.iso_file",
            validation.map(|v| &v.iso_file),
        );
    } else {
        push_field(lines, "error", "Missing [proxmox] section");
    }
}

/// Push an editable field with an optional validation status suffix dot.
fn push_validated_field(
    lines: &mut Vec<DetailLine>,
    label: &str,
    value: &str,
    file: &'static str,
    path: &str,
    status: Option<&FieldStatus>,
) {
    let p = Palette::default();
    let suffix = status.map(|s| match s {
        FieldStatus::Unknown => ("●".to_string(), p.text_disabled),
        FieldStatus::Checking => ("●".to_string(), p.text_disabled),
        FieldStatus::Valid => ("●".to_string(), p.green_primary),
        FieldStatus::Invalid(_) => ("●".to_string(), p.red_error),
    });

    lines.push(DetailLine {
        text: format!("    {label}: {value}"),
        style: config_detail::DetailStyle::EditableField,
        field: Some(config_detail::FieldKey {
            file,
            path: path.to_string(),
        }),
        raw_value: Some(value.to_string()),
        picker: None,
        action: None,
        fg_override: None,
        suffix,
    });
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
        if matches!(self.edit_mode, EditMode::PickerSelect { .. }) {
            return self.handle_picker_key(key);
        }

        // Inline text editing
        if let EditMode::Editing { .. } = self.edit_mode {
            match key.code {
                KeyCode::Enter => match self.confirm_edit() {
                    Err(msg) => return PanelEvent::Action(PanelAction::Error(msg)),
                    Ok(Some(action)) => return PanelEvent::Action(action),
                    Ok(None) => {}
                },
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
            let modified_lines: Vec<DetailLine> = self
                .lines
                .iter()
                .map(|line| {
                    if line.action == Some(LineAction::DeleteHypervisor) {
                        DetailLine {
                            text: format!(
                                "  Delete {}? Enter to confirm, Esc to cancel",
                                self.name
                            ),
                            style: DetailStyle::DangerAction,
                            action: Some(LineAction::DeleteHypervisor),
                            ..line.clone()
                        }
                    } else {
                        line.clone()
                    }
                })
                .collect();

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
        if let EditMode::PickerSelect { ref picker } = self.edit_mode {
            picker.render(frame, area, p);
        }
    }

    fn receive_data(&mut self, data: PanelData) {
        match data {
            PanelData::SshProbeResult(status) => {
                self.ssh_status = status;
                // If SSH is valid and this is a proxmox hypervisor, trigger validation
                // by setting validation to "checking" — the actual trigger happens
                // via PanelAction returned from request_probe's caller chain.
                self.rebuild_lines();
            }
            PanelData::ProxmoxValidation(validation) => {
                // Auto-fix node case if needed
                if let Some(ref correct_node) = validation.node_auto_fix {
                    let _ = update_hypervisor_field(&self.name, "proxmox.node", correct_node);
                    self.reload();
                }
                // If validation shows ISO is now valid, clear download state
                if matches!(validation.iso_file, FieldStatus::Valid) {
                    self.download_state = DownloadState::Idle;
                }
                self.proxmox_validation = Some(validation);
                self.rebuild_lines();
            }
            PanelData::DownloadProgress { percent } => {
                self.download_state = DownloadState::Downloading { percent };
                self.rebuild_lines();
            }
            PanelData::DownloadComplete => {
                self.download_state = DownloadState::Idle;
                // Re-validation will be triggered by App
                self.rebuild_lines();
            }
            PanelData::DownloadFailed(msg) => {
                self.download_state = DownloadState::Failed(msg);
                self.rebuild_lines();
            }
            _ => {}
        }
    }
}
