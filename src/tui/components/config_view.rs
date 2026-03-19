use ratatui::prelude::*;
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

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

#[derive(Debug, Clone, PartialEq, Eq)]
enum EditMode {
    Viewing,
    Editing { buffer: String, cursor: usize },
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

    // Right panel navigation & editing
    selected_line: usize,
    scroll_offset: usize,
    edit_mode: EditMode,
}

impl ConfigView {
    pub fn new() -> Self {
        let deployments = list_deployments().unwrap_or_default();
        let hypervisors = list_hypervisors().unwrap_or_default();

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
            selected_line: 0,
            scroll_offset: 0,
            edit_mode: EditMode::Viewing,
        };

        if !view.deployments.is_empty() {
            view.deployment_list_state.select(Some(0));
            view.load_selected_deployment();
        }

        view
    }

    pub fn is_editing(&self) -> bool {
        matches!(self.edit_mode, EditMode::Editing { .. })
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
        if self.focus == ConfigFocus::LeftPanel {
            // Enter on left panel = switch to right panel, select first editable field
            self.selected_line = self.first_editable_line();
            self.scroll_offset = 0;
            self.focus = ConfigFocus::RightPanel;
            return;
        }
        if let Some(line) = self.detail_lines.get(self.selected_line) {
            if line.field.is_some() {
                let value = line.raw_value.clone().unwrap_or_default();
                let cursor = value.chars().count();
                self.edit_mode = EditMode::Editing {
                    buffer: value,
                    cursor,
                };
            }
        }
    }

    pub fn cancel_edit(&mut self) {
        self.edit_mode = EditMode::Viewing;
    }

    /// Confirm the edit, persist to disk, reload. Returns Ok(()) on success,
    /// or Err(message) if the save failed.
    pub fn confirm_edit(&mut self) -> Result<(), String> {
        let EditMode::Editing { ref buffer, .. } = self.edit_mode else {
            return Ok(());
        };
        let new_value = buffer.clone();
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

    // Cursor is a *char index* (not byte index) for UTF-8 safety.
    // Use `char_to_byte()` to convert to byte offset for slicing.

    pub fn edit_insert_char(&mut self, c: char) {
        if let EditMode::Editing { ref mut buffer, ref mut cursor } = self.edit_mode {
            let byte_pos = char_to_byte(buffer, *cursor);
            buffer.insert(byte_pos, c);
            *cursor += 1;
        }
    }

    pub fn edit_backspace(&mut self) {
        if let EditMode::Editing { ref mut buffer, ref mut cursor } = self.edit_mode {
            if *cursor > 0 {
                *cursor -= 1;
                let byte_pos = char_to_byte(buffer, *cursor);
                buffer.remove(byte_pos);
            }
        }
    }

    pub fn edit_delete(&mut self) {
        if let EditMode::Editing { ref mut buffer, ref cursor } = self.edit_mode {
            let char_count = buffer.chars().count();
            if *cursor < char_count {
                let byte_pos = char_to_byte(buffer, *cursor);
                buffer.remove(byte_pos);
            }
        }
    }

    pub fn edit_move_left(&mut self) {
        if let EditMode::Editing { ref mut cursor, .. } = self.edit_mode {
            *cursor = cursor.saturating_sub(1);
        }
    }

    pub fn edit_move_right(&mut self) {
        if let EditMode::Editing { ref buffer, ref mut cursor } = self.edit_mode {
            let char_count = buffer.chars().count();
            if *cursor < char_count {
                *cursor += 1;
            }
        }
    }

    pub fn edit_home(&mut self) {
        if let EditMode::Editing { ref mut cursor, .. } = self.edit_mode {
            *cursor = 0;
        }
    }

    pub fn edit_end(&mut self) {
        if let EditMode::Editing { ref buffer, ref mut cursor } = self.edit_mode {
            *cursor = buffer.chars().count();
        }
    }

    // --- Rendering ---

    pub fn render(&mut self, frame: &mut Frame, area: Rect) {
        let p = Palette::default();

        let layout = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(24), Constraint::Min(40)])
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
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(4), Constraint::Length(1), Constraint::Min(3)])
                .split(inner)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(4), Constraint::Length(0), Constraint::Length(0)])
                .split(inner)
        };

        let items: Vec<ListItem> = self
            .deployments
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let selected = self.section == ConfigSection::Deployments
                    && self.deployment_list_state.selected() == Some(i);
                let marker = if selected { " > " } else { "   " };
                let style = if selected {
                    Style::default().fg(p.green_primary).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(p.text_default)
                };
                ListItem::new(format!("{marker}{name}")).style(style)
            })
            .collect();
        frame.render_widget(List::new(items), sections[0]);

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
                .enumerate()
                .map(|(i, name)| {
                    let selected = self.section == ConfigSection::Hypervisors
                        && self.hypervisor_list_state.selected() == Some(i);
                    let marker = if selected { " > " } else { "   " };
                    let style = if selected {
                        Style::default().fg(p.green_primary)
                    } else {
                        Style::default().fg(p.text_tertiary)
                    };
                    ListItem::new(format!("{marker}{name}")).style(style)
                })
                .collect();
            frame.render_widget(List::new(hyp_items), sections[2]);
        }
    }

    fn render_right_panel(&mut self, frame: &mut Frame, area: Rect, p: &Palette) {
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

        let visible_height = inner.height as usize;

        // Adjust scroll to keep selected_line visible and persist it
        self.scroll_offset = if self.selected_line >= self.scroll_offset + visible_height {
            self.selected_line - visible_height + 1
        } else if self.selected_line < self.scroll_offset {
            self.selected_line
        } else {
            self.scroll_offset
        };

        let lines: Vec<Line> = self
            .detail_lines
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(visible_height)
            .map(|(i, dl)| {
                let is_selected = i == self.selected_line && self.focus == ConfigFocus::RightPanel;
                let is_editing = is_selected && matches!(self.edit_mode, EditMode::Editing { .. });

                if is_editing {
                    if let EditMode::Editing { ref buffer, cursor } = self.edit_mode {
                        // Render edit line: label + editable buffer with cursor
                        // cursor is a char index — convert to byte boundaries for slicing
                        let label = dl.text.split(':').next().unwrap_or("    ?");
                        let byte_cursor = char_to_byte(buffer, cursor);
                        let before = &buffer[..byte_cursor];
                        let cursor_str: String = buffer.chars().nth(cursor)
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| " ".to_string());
                        let byte_after = char_to_byte(buffer, cursor + 1);
                        let after = &buffer[byte_after.min(buffer.len())..];

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

        frame.render_widget(
            Paragraph::new(lines).style(Style::default().bg(p.bg_panel)),
            inner,
        );
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

/// Convert a char index to a byte offset in a string.
/// Returns `s.len()` if `char_idx` is at or past the end.
fn char_to_byte(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(byte, _)| byte)
        .unwrap_or(s.len())
}
