use std::cell::Cell;

use crossterm::event::Event as CtEvent;
use ratatui::prelude::*;
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::Component;
use crate::config::editor::update_deployment_field;
use crate::config::loader::{
    list_deployments, list_hypervisors, load_deployment, load_deployment_state, load_hypervisor,
};
use crate::config::types::{DeploymentConfig, DeploymentState, HypervisorConfig};
use crate::tui::theme::{panel_block, Palette};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigSection {
    Deployments,
    Hypervisors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFocus {
    LeftPanel,
    RightPanel,
}

enum EditMode {
    Viewing,
    Editing { input: Input },
}

/// Identifies a field's location in the TOML files for editing.
#[derive(Debug, Clone)]
struct FieldKey {
    /// Which TOML file: "deployment" or "build"
    file: &'static str,
    /// Dotted path into the TOML, e.g. "network.gateway" or "omicron.rust_toolchain"
    path: String,
}

struct DetailLine {
    text: String,
    style: DetailStyle,
    /// If set, this line is an editable field
    field: Option<FieldKey>,
    /// The raw value (without label prefix) for populating edit buffer
    raw_value: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DetailStyle {
    SectionHeader,
    Field,
    EditableField,
}

pub struct ConfigView {
    // Left panel state
    deployments: Vec<String>,
    hypervisors: Vec<String>,
    section: ConfigSection,
    deployment_list_state: ListState,
    hypervisor_list_state: ListState,
    pub focus: ConfigFocus,

    // Right panel: loaded config for selected deployment
    active_config: Option<DeploymentConfig>,
    active_hypervisor: Option<HypervisorConfig>,
    active_state: Option<DeploymentState>,
    active_name: Option<String>,
    detail_lines: Vec<DetailLine>,

    // The deployment that Build/Monitor/Recovery actually use
    activated_name: Option<String>,

    // Right panel navigation & editing
    selected_line: usize,
    scroll_offset: usize,
    edit_mode: EditMode,

    // Tracks right panel height from last render for scroll calculations
    visible_height: Cell<usize>,
}

impl ConfigView {
    pub fn new(initial_deployment: &str) -> Self {
        let deployments = list_deployments().unwrap_or_default();
        let hypervisors = list_hypervisors().unwrap_or_default();

        // Find the index of the initial deployment, fall back to 0
        let initial_idx = deployments
            .iter()
            .position(|n| n == initial_deployment)
            .unwrap_or(0);

        let mut view = Self {
            deployments,
            hypervisors,
            section: ConfigSection::Deployments,
            deployment_list_state: ListState::default(),
            hypervisor_list_state: ListState::default(),
            focus: ConfigFocus::LeftPanel,
            active_config: None,
            active_hypervisor: None,
            active_state: None,
            active_name: None,
            detail_lines: Vec::new(),
            activated_name: Some(initial_deployment.to_string()),
            selected_line: 0,
            scroll_offset: 0,
            edit_mode: EditMode::Viewing,
            visible_height: Cell::new(0),
        };

        if !view.deployments.is_empty() {
            view.deployment_list_state.select(Some(initial_idx));
            view.load_selected_deployment();
        }

        view
    }

    pub fn is_editing(&self) -> bool {
        matches!(self.edit_mode, EditMode::Editing { .. })
    }

    pub fn is_left_panel_focused(&self) -> bool {
        self.focus == ConfigFocus::LeftPanel
    }

    pub fn selected_deployment_name(&self) -> Option<&str> {
        self.active_name.as_deref()
    }

    pub fn activated_name(&self) -> Option<&str> {
        self.activated_name.as_deref()
    }

    pub fn activated_config(&self) -> Option<&DeploymentConfig> {
        if self.active_name == self.activated_name {
            self.active_config.as_ref()
        } else {
            None
        }
    }

    /// Attempt to activate the currently selected deployment.
    /// Returns the name and config if activation succeeded.
    pub fn activate_selected(&mut self) -> Option<(&str, &DeploymentConfig)> {
        if let (Some(name), Some(_config)) = (&self.active_name, &self.active_config) {
            self.activated_name = Some(name.clone());
            // Re-borrow to satisfy the borrow checker
            let name = self.activated_name.as_deref().unwrap();
            let config = self.active_config.as_ref().unwrap();
            Some((name, config))
        } else {
            None
        }
    }

    pub fn refresh_lists(&mut self) {
        self.deployments = list_deployments().unwrap_or_default();
        self.hypervisors = list_hypervisors().unwrap_or_default();
    }

    fn load_selected_deployment(&mut self) {
        if let Some(idx) = self.deployment_list_state.selected() {
            if let Some(name) = self.deployments.get(idx) {
                match load_deployment(name) {
                    Ok(config) => {
                        let state = load_deployment_state(name).unwrap_or_default();
                        let hyp = config
                            .deployment
                            .hypervisor
                            .as_ref()
                            .and_then(|href| load_hypervisor(&href.hypervisor_ref).ok());

                        self.active_name = Some(name.clone());
                        self.active_config = Some(config);
                        self.active_state = Some(state);
                        self.active_hypervisor = hyp;
                        self.selected_line = 0;
                        self.scroll_offset = 0;
                        self.edit_mode = EditMode::Viewing;
                        self.rebuild_detail_lines();
                    }
                    Err(e) => {
                        self.active_config = None;
                        self.active_state = None;
                        self.active_hypervisor = None;
                        self.active_name = None;
                        self.detail_lines = vec![DetailLine {
                            text: format!("  Error loading: {e}"),
                            style: DetailStyle::Field,
                            field: None,
                            raw_value: None,
                        }];
                    }
                }
            }
        }
    }

    fn rebuild_detail_lines(&mut self) {
        let Some(config) = &self.active_config else {
            self.detail_lines.clear();
            return;
        };

        let mut lines = Vec::new();
        let d = &config.deployment;
        let b = &config.build;

        // -- Hosts section --
        push_header(&mut lines, "HOSTS");
        for (name, host) in &d.hosts {
            push_editable(
                &mut lines,
                &format!("{name}.address"),
                &host.address,
                "deployment",
                &format!("hosts.{name}.address"),
            );
            push_editable(
                &mut lines,
                &format!("{name}.ssh_user"),
                &host.ssh_user,
                "deployment",
                &format!("hosts.{name}.ssh_user"),
            );
        }

        // -- Network section --
        push_header(&mut lines, "NETWORK");
        push_editable(&mut lines, "gateway", &d.network.gateway, "deployment", "network.gateway");
        push_editable(&mut lines, "infra_ip", &d.network.infra_ip, "deployment", "network.infra_ip");
        push_field(&mut lines, "dns_ips", &d.network.external_dns_ips.join(", "));
        push_editable(
            &mut lines,
            "services_first",
            &d.network.internal_services_range.first,
            "deployment",
            "network.internal_services_range.first",
        );
        push_editable(
            &mut lines,
            "services_last",
            &d.network.internal_services_range.last,
            "deployment",
            "network.internal_services_range.last",
        );
        push_editable(
            &mut lines,
            "pool_first",
            &d.network.instance_pool_range.first,
            "deployment",
            "network.instance_pool_range.first",
        );
        push_editable(
            &mut lines,
            "pool_last",
            &d.network.instance_pool_range.last,
            "deployment",
            "network.instance_pool_range.last",
        );

        // -- Build section --
        push_header(&mut lines, "BUILD");
        push_editable(&mut lines, "omicron_path", &b.omicron.repo_path, "build", "omicron.repo_path");
        push_editable(
            &mut lines,
            "repo_url",
            b.omicron.repo_url.as_deref().unwrap_or(""),
            "build",
            "omicron.repo_url",
        );
        push_editable(
            &mut lines,
            "git_ref",
            b.omicron.git_ref.as_deref().unwrap_or(""),
            "build",
            "omicron.git_ref",
        );
        push_editable(
            &mut lines,
            "rust_toolchain",
            b.omicron.rust_toolchain.as_deref().unwrap_or(""),
            "build",
            "omicron.rust_toolchain",
        );

        // -- Overrides section --
        push_header(&mut lines, "OVERRIDES");
        push_editable(
            &mut lines,
            "crdb_redundancy",
            &b.omicron.overrides.cockroachdb_redundancy.map(|v| v.to_string()).unwrap_or_default(),
            "build",
            "omicron.overrides.cockroachdb_redundancy",
        );
        push_editable(
            &mut lines,
            "vdev_count",
            &b.omicron.overrides.vdev_count.map(|v| v.to_string()).unwrap_or_default(),
            "build",
            "omicron.overrides.vdev_count",
        );
        push_editable(
            &mut lines,
            "storage_buffer_gib",
            &b.omicron.overrides.control_plane_storage_buffer_gib.map(|v| v.to_string()).unwrap_or_default(),
            "build",
            "omicron.overrides.control_plane_storage_buffer_gib",
        );

        // -- Propolis section --
        if let Some(propolis) = &b.propolis {
            push_header(&mut lines, "PROPOLIS");
            push_field(
                &mut lines,
                "patched",
                &propolis.patched.map(|v| v.to_string()).unwrap_or_else(|| "(not set)".into()),
            );
            push_editable(
                &mut lines,
                "patch_type",
                propolis.patch_type.as_deref().unwrap_or(""),
                "build",
                "propolis.patch_type",
            );
            push_field(
                &mut lines,
                "source",
                &propolis.source.as_ref().map(|s| format!("{s:?}")).unwrap_or_else(|| "(not set)".into()),
            );
            push_editable(
                &mut lines,
                "repo_url",
                propolis.repo_url.as_deref().unwrap_or(""),
                "build",
                "propolis.repo_url",
            );
        }

        // -- VM section --
        if let Some(px) = &d.proxmox {
            push_header(&mut lines, "VM (PROXMOX)");
            push_field(&mut lines, "host", &px.host);
            push_field(&mut lines, "vmid", &px.vm.vmid.to_string());
            push_field(&mut lines, "name", &px.vm.name);
            push_field(&mut lines, "cores", &format!("{} x {} sockets", px.vm.cores, px.vm.sockets));
            push_field(&mut lines, "memory", &format!("{} MB", px.vm.memory_mb));
            push_field(&mut lines, "disk", &format!("{} GB ({})", px.vm.disk_gb, px.vm.disk_bus));
        } else if let Some(href) = &d.hypervisor {
            if let Some(vm) = &href.vm {
                push_header(&mut lines, "VM");
                push_field(&mut lines, "hypervisor", &href.hypervisor_ref);
                push_editable(&mut lines, "vmid", &vm.vmid.to_string(), "deployment", "hypervisor.vm.vmid");
                push_editable(&mut lines, "name", &vm.name, "deployment", "hypervisor.vm.name");
                push_editable(
                    &mut lines,
                    "cores",
                    &vm.cores.to_string(),
                    "deployment",
                    "hypervisor.vm.cores",
                );
                push_editable(
                    &mut lines,
                    "sockets",
                    &vm.sockets.to_string(),
                    "deployment",
                    "hypervisor.vm.sockets",
                );
                push_editable(
                    &mut lines,
                    "memory_mb",
                    &vm.memory_mb.to_string(),
                    "deployment",
                    "hypervisor.vm.memory_mb",
                );
                push_editable(
                    &mut lines,
                    "disk_gb",
                    &vm.disk_gb.to_string(),
                    "deployment",
                    "hypervisor.vm.disk_gb",
                );
            }
        }

        // -- Drift section --
        if let Some(state) = &self.active_state {
            if let Some(drift) = &state.drift {
                push_header(&mut lines, "DRIFT");
                push_field(&mut lines, "last_checked", &drift.last_checked);
            }
        }

        self.detail_lines = lines;
    }

    // --- Navigation ---

    pub fn navigate_up(&mut self) {
        match self.focus {
            ConfigFocus::LeftPanel => self.left_panel_up(),
            ConfigFocus::RightPanel => {
                // Skip to previous editable field
                if let Some(prev) = self.prev_editable_line(self.selected_line) {
                    self.selected_line = prev;
                    self.ensure_visible();
                }
            }
        }
    }

    pub fn navigate_down(&mut self) {
        match self.focus {
            ConfigFocus::LeftPanel => self.left_panel_down(),
            ConfigFocus::RightPanel => {
                // Skip to next editable field
                if let Some(next) = self.next_editable_line(self.selected_line) {
                    self.selected_line = next;
                    self.ensure_visible();
                }
            }
        }
    }

    fn next_editable_line(&self, from: usize) -> Option<usize> {
        (from + 1..self.detail_lines.len())
            .find(|&i| self.detail_lines[i].field.is_some())
    }

    fn prev_editable_line(&self, from: usize) -> Option<usize> {
        (0..from)
            .rev()
            .find(|&i| self.detail_lines[i].field.is_some())
    }

    fn first_editable_line(&self) -> usize {
        self.detail_lines
            .iter()
            .position(|l| l.field.is_some())
            .unwrap_or(0)
    }

    fn ensure_visible(&mut self) {
        if self.selected_line < self.scroll_offset {
            self.scroll_offset = self.selected_line;
        }
        let h = self.visible_height.get();
        if h > 0 && self.selected_line >= self.scroll_offset + h {
            self.scroll_offset = self.selected_line - h + 1;
        }
    }

    fn left_panel_up(&mut self) {
        match self.section {
            ConfigSection::Deployments => {
                if let Some(idx) = self.deployment_list_state.selected() {
                    if idx > 0 {
                        self.deployment_list_state.select(Some(idx - 1));
                        self.load_selected_deployment();
                    } else if !self.hypervisors.is_empty() {
                        self.section = ConfigSection::Hypervisors;
                        self.hypervisor_list_state.select(Some(self.hypervisors.len() - 1));
                    }
                }
            }
            ConfigSection::Hypervisors => {
                if let Some(idx) = self.hypervisor_list_state.selected() {
                    if idx > 0 {
                        self.hypervisor_list_state.select(Some(idx - 1));
                    } else if !self.deployments.is_empty() {
                        self.section = ConfigSection::Deployments;
                        self.deployment_list_state.select(Some(self.deployments.len() - 1));
                        self.load_selected_deployment();
                    }
                }
            }
        }
    }

    fn left_panel_down(&mut self) {
        match self.section {
            ConfigSection::Deployments => {
                if let Some(idx) = self.deployment_list_state.selected() {
                    if idx + 1 < self.deployments.len() {
                        self.deployment_list_state.select(Some(idx + 1));
                        self.load_selected_deployment();
                    } else if !self.hypervisors.is_empty() {
                        self.section = ConfigSection::Hypervisors;
                        self.hypervisor_list_state.select(Some(0));
                    }
                }
            }
            ConfigSection::Hypervisors => {
                if let Some(idx) = self.hypervisor_list_state.selected() {
                    if idx + 1 < self.hypervisors.len() {
                        self.hypervisor_list_state.select(Some(idx + 1));
                    } else if !self.deployments.is_empty() {
                        self.section = ConfigSection::Deployments;
                        self.deployment_list_state.select(Some(0));
                        self.load_selected_deployment();
                    }
                }
            }
        }
    }

    pub fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            ConfigFocus::LeftPanel => {
                self.selected_line = self.first_editable_line();
                self.scroll_offset = 0;
                ConfigFocus::RightPanel
            }
            ConfigFocus::RightPanel => ConfigFocus::LeftPanel,
        };
    }

    // --- Editing ---

    pub fn start_edit(&mut self) {
        if let Some(line) = self.detail_lines.get(self.selected_line) {
            if line.field.is_some() {
                let value = line.raw_value.clone().unwrap_or_default();
                let input = Input::new(value);
                self.edit_mode = EditMode::Editing { input };
            }
        }
    }

    pub fn cancel_edit(&mut self) {
        self.edit_mode = EditMode::Viewing;
    }

    /// Confirm the edit, persist to disk, reload. Returns Ok(()) on success,
    /// or Err(message) if the save failed.
    pub fn confirm_edit(&mut self) -> Result<(), String> {
        let EditMode::Editing { ref input } = self.edit_mode else {
            return Ok(());
        };
        let new_value = input.value().to_string();
        self.edit_mode = EditMode::Viewing;

        let Some(name) = &self.active_name else { return Ok(()) };
        let Some(line) = self.detail_lines.get(self.selected_line) else { return Ok(()) };
        let Some(field_key) = &line.field else { return Ok(()) };

        // Persist to TOML
        let name = name.clone();
        let file = field_key.file;
        let path = field_key.path.clone();

        if let Err(e) = update_deployment_field(&name, file, &path, &new_value) {
            let msg = format!("Failed to save {path}: {e}");
            tracing::error!("{msg}");
            // Reload to restore original state
            self.load_selected_deployment();
            return Err(msg);
        }

        // Reload config to reflect changes
        self.load_selected_deployment();
        Ok(())
    }

    /// Forward a crossterm event to the tui-input handler.
    pub fn handle_edit_event(&mut self, event: &CtEvent) {
        if let EditMode::Editing { ref mut input } = self.edit_mode {
            input.handle_event(event);
        }
    }

    pub fn is_right_panel_focused(&self) -> bool {
        self.focus == ConfigFocus::RightPanel
    }

    // --- Rendering ---

    fn render_impl(&self, frame: &mut Frame, area: Rect) {
        let p = Palette::default();

        let layout = Layout::horizontal([Constraint::Length(24), Constraint::Min(40)])
            .split(area);

        self.render_left_panel(frame, layout[0], &p);
        self.render_right_panel(frame, layout[1], &p);
    }

    fn render_left_panel(&self, frame: &mut Frame, area: Rect, p: &Palette) {
        let focused = self.focus == ConfigFocus::LeftPanel;
        let block = panel_block("Deployments", focused, p);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let has_hypervisors = !self.hypervisors.is_empty();
        let sections = if has_hypervisors {
            Layout::vertical([Constraint::Min(4), Constraint::Length(1), Constraint::Min(3)])
                .split(inner)
        } else {
            Layout::vertical([Constraint::Min(4), Constraint::Length(0), Constraint::Length(0)])
                .split(inner)
        };

        let items: Vec<ListItem> = self
            .deployments
            .iter()
            .map(|name| {
                let is_activated = self.activated_name.as_deref() == Some(name.as_str());
                if is_activated {
                    ListItem::new(Line::from(vec![
                        Span::styled(name.as_str(), Style::default().fg(p.green_primary)),
                        Span::styled(" ●", Style::default().fg(p.green_primary)),
                    ]))
                } else {
                    ListItem::new(name.as_str())
                        .style(Style::default().fg(p.text_default))
                }
            })
            .collect();
        let deploy_list = List::new(items)
            .highlight_style(Style::default().fg(p.text_bright).add_modifier(Modifier::BOLD))
            .highlight_symbol(" > ")
            .highlight_spacing(ratatui::widgets::HighlightSpacing::Always);
        if self.section == ConfigSection::Deployments {
            let mut state = self.deployment_list_state.clone();
            frame.render_stateful_widget(deploy_list, sections[0], &mut state);
        } else {
            frame.render_widget(deploy_list, sections[0]);
        }

        if has_hypervisors {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled(
                    " HYPERVISORS",
                    Style::default().fg(p.text_bright).add_modifier(Modifier::BOLD),
                )))
                .style(Style::default().bg(p.bg_panel)),
                sections[1],
            );

            let hyp_items: Vec<ListItem> = self
                .hypervisors
                .iter()
                .map(|name| ListItem::new(name.as_str()).style(Style::default().fg(p.text_tertiary)))
                .collect();
            let hyp_list = List::new(hyp_items)
                .highlight_style(Style::default().fg(p.green_primary))
                .highlight_symbol(" > ")
                .highlight_spacing(ratatui::widgets::HighlightSpacing::Always);
            if self.section == ConfigSection::Hypervisors {
                let mut state = self.hypervisor_list_state.clone();
                frame.render_stateful_widget(hyp_list, sections[2], &mut state);
            } else {
                frame.render_widget(hyp_list, sections[2]);
            }
        }
    }

    fn render_right_panel(&self, frame: &mut Frame, area: Rect, p: &Palette) {
        let focused = self.focus == ConfigFocus::RightPanel;
        let title = self.active_name.as_deref().unwrap_or("No deployment selected");
        let block = panel_block(title, focused, p);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.detail_lines.is_empty() {
            frame.render_widget(
                Paragraph::new("  Select a deployment to view its configuration.")
                    .style(Style::default().fg(p.text_tertiary).bg(p.bg_panel)),
                inner,
            );
            return;
        }

        // Store visible height for scroll calculations in ensure_visible()
        self.visible_height.set(inner.height as usize);

        let lines: Vec<Line> = self
            .detail_lines
            .iter()
            .enumerate()
            .map(|(i, dl)| {
                let is_selected = i == self.selected_line && self.focus == ConfigFocus::RightPanel;
                let is_editing = is_selected && matches!(self.edit_mode, EditMode::Editing { .. });

                if is_editing {
                    if let EditMode::Editing { ref input } = self.edit_mode {
                        let label = dl.text.split(':').next().unwrap_or("    ?");
                        let value = input.value();
                        let cursor = input.visual_cursor();
                        let (before, rest) = value.split_at(
                            value.char_indices()
                                .nth(cursor)
                                .map(|(b, _)| b)
                                .unwrap_or(value.len()),
                        );
                        let cursor_str: String = rest.chars().next()
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| " ".to_string());
                        let after_start = cursor_str.len().min(rest.len());
                        let after = &rest[after_start..];

                        return Line::from(vec![
                            Span::styled(format!("{label}: "), Style::default().fg(p.green_primary)),
                            Span::styled(before.to_string(), Style::default().fg(p.text_bright).bg(p.bg_hover)),
                            Span::styled(
                                cursor_str,
                                Style::default().fg(p.bg_base).bg(p.text_bright),
                            ),
                            Span::styled(after.to_string(), Style::default().fg(p.text_bright).bg(p.bg_hover)),
                        ]);
                    }
                }

                let base_style = match dl.style {
                    DetailStyle::SectionHeader => Style::default()
                        .fg(p.text_bright)
                        .add_modifier(Modifier::BOLD),
                    DetailStyle::Field => Style::default().fg(p.text_tertiary),
                    DetailStyle::EditableField => Style::default().fg(p.text_default),
                };

                if is_selected {
                    let highlight = if dl.field.is_some() {
                        base_style.bg(p.bg_hover)
                    } else {
                        base_style
                    };
                    Line::from(Span::styled(&dl.text, highlight))
                } else {
                    Line::from(Span::styled(&dl.text, base_style))
                }
            })
            .collect();

        let total_lines = lines.len();
        let visible_height = inner.height as usize;
        frame.render_widget(
            Paragraph::new(lines)
                .style(Style::default().bg(p.bg_panel))
                .scroll((self.scroll_offset as u16, 0)),
            inner,
        );

        // Only show scrollbar when content overflows
        if total_lines > visible_height {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(p.border_default));
            let mut scrollbar_state = ScrollbarState::new(total_lines.saturating_sub(visible_height))
                .position(self.scroll_offset);
            frame.render_stateful_widget(scrollbar, inner, &mut scrollbar_state);
        }
    }
}

impl Component for ConfigView {
    fn render(&self, frame: &mut Frame, area: Rect) {
        self.render_impl(frame, area);
    }
}

// --- Free functions for building detail lines ---

fn push_header(lines: &mut Vec<DetailLine>, title: &str) {
    lines.push(DetailLine {
        text: String::new(),
        style: DetailStyle::SectionHeader,
        field: None,
        raw_value: None,
    });
    lines.push(DetailLine {
        text: format!("  {title}"),
        style: DetailStyle::SectionHeader,
        field: None,
        raw_value: None,
    });
}

fn push_field(lines: &mut Vec<DetailLine>, label: &str, value: &str) {
    lines.push(DetailLine {
        text: format!("    {label}: {value}"),
        style: DetailStyle::Field,
        field: None,
        raw_value: None,
    });
}

fn push_editable(
    lines: &mut Vec<DetailLine>,
    label: &str,
    value: &str,
    file: &'static str,
    path: &str,
) {
    lines.push(DetailLine {
        text: format!("    {label}: {value}"),
        style: DetailStyle::EditableField,
        field: Some(FieldKey {
            file,
            path: path.to_string(),
        }),
        raw_value: Some(value.to_string()),
    });
}
