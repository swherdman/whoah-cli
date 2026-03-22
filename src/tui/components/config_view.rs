use ratatui::prelude::*;
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use crossterm::event::{KeyCode, KeyEvent};

use super::Component;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::config_detail::{self, ConfigPanel, PanelAction, PanelData, PanelEvent};
use super::deployment_panel::DeploymentPanel;
use super::hypervisor_panel::HypervisorPanel;
use super::popup_picker::{PopupAction, PopupPicker};
use crate::config::editor::create_hypervisor;
use crate::ssh::probe::SshProbeStatus;
use crate::config::loader::{
    find_referencing_deployments, list_deployments, list_hypervisors, load_deployment,
    load_deployment_state, load_hypervisor,
};
use crate::config::types::{DeploymentConfig, HypervisorType};
use crate::tui::theme::{panel_block, Palette};

// ── Types ──────────────────────────────────────────────────────────────

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

/// What ConfigView returns to App after handling a key.
pub enum ConfigViewEvent {
    /// Key was consumed internally.
    Consumed,
    /// Key was not handled — App should route to globals.
    Ignored,
    /// Request to activate a deployment. App should validate (no build/recovery running).
    RequestActivation { name: String },
    /// Request async git ref fetch.
    FetchGitRefs { repo_url: String },
    /// Request SSH credential probe.
    ProbeSsh { host: String, user: String },
    /// Request Proxmox config validation.
    ValidateProxmox { host: String, user: String },
    /// A hypervisor was deleted.
    HypervisorDeleted { name: String },
}

/// Right panel content — one variant per config type.
enum ActivePanel {
    Deployment(DeploymentPanel),
    Hypervisor(HypervisorPanel),
    Prompt(String),
}

/// State machine for the "Add hypervisor" flow.
enum CreateFlow {
    Inactive,
    /// User is picking the hypervisor type.
    TypeSelection { picker: PopupPicker },
    /// User is entering the hypervisor name.
    NameInput { htype: HypervisorType, input: Input },
}

// ── ConfigView ─────────────────────────────────────────────────────────

pub struct ConfigView {
    // Left panel
    deployments: Vec<String>,
    hypervisors: Vec<String>,
    section: ConfigSection,
    deployment_list_state: ListState,
    hypervisor_list_state: ListState,
    pub focus: ConfigFocus,

    // Right panel
    active_panel: ActivePanel,

    // Add hypervisor flow
    create_flow: CreateFlow,

    // Activation state — persists across panel switches
    activated_name: Option<String>,
    activated_config: Option<DeploymentConfig>,
}

impl ConfigView {
    pub fn new(initial_deployment: &str) -> Self {
        let deployments = list_deployments().unwrap_or_default();
        let hypervisors = list_hypervisors().unwrap_or_default();

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
            active_panel: ActivePanel::Prompt("Select a deployment to view its configuration.".into()),
            create_flow: CreateFlow::Inactive,
            activated_name: Some(initial_deployment.to_string()),
            activated_config: None,
        };

        if !view.deployments.is_empty() {
            view.deployment_list_state.select(Some(initial_idx));
            view.load_selected_deployment();
            // Cache the initial activation config
            if let ActivePanel::Deployment(ref panel) = view.active_panel {
                view.activated_config = Some(panel.config().clone());
            }
        }

        view
    }

    // --- Public interface for App ---

    /// Handle a key event. ConfigView routes internally and returns an event for App.
    pub fn handle_key(&mut self, key: KeyEvent) -> ConfigViewEvent {
        // 1. If active panel is capturing (editing/picking), delegate everything to it
        {
            let is_capturing = match &self.active_panel {
                ActivePanel::Deployment(p) => p.is_capturing(),
                ActivePanel::Hypervisor(p) => p.is_capturing(),
                ActivePanel::Prompt(_) => false,
            };
            if is_capturing {
                let event = match &mut self.active_panel {
                    ActivePanel::Deployment(p) => p.handle_key(key),
                    ActivePanel::Hypervisor(p) => p.handle_key(key),
                    ActivePanel::Prompt(_) => PanelEvent::Ignored,
                };
                return self.dispatch_panel_event(event);
            }
        }

        // 2. If create flow is active, it captures all input
        if !matches!(self.create_flow, CreateFlow::Inactive) {
            return self.handle_create_flow_key(key);
        }

        // 3. Normal routing
        match key.code {
            KeyCode::Esc => {
                if self.focus == ConfigFocus::RightPanel {
                    self.focus = ConfigFocus::LeftPanel;
                    ConfigViewEvent::Consumed
                } else {
                    ConfigViewEvent::Ignored // App handles screen switch
                }
            }
            KeyCode::Tab => {
                self.toggle_focus().unwrap_or(ConfigViewEvent::Consumed)
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.focus == ConfigFocus::LeftPanel {
                    self.left_panel_down();
                    ConfigViewEvent::Consumed
                } else if let Some(panel) = self.active_panel_mut() {
                    let event = panel.handle_key(key);
                    self.dispatch_panel_event(event)
                } else {
                    ConfigViewEvent::Consumed
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.focus == ConfigFocus::LeftPanel {
                    self.left_panel_up();
                    ConfigViewEvent::Consumed
                } else if let Some(panel) = self.active_panel_mut() {
                    let event = panel.handle_key(key);
                    self.dispatch_panel_event(event)
                } else {
                    ConfigViewEvent::Consumed
                }
            }
            KeyCode::Enter => self.handle_enter(key),
            KeyCode::Char('e') => {
                if self.focus == ConfigFocus::LeftPanel {
                    return self.toggle_focus().unwrap_or(ConfigViewEvent::Consumed);
                }
                ConfigViewEvent::Consumed
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if self.focus == ConfigFocus::RightPanel {
                    if let Some(panel) = self.active_panel_mut() {
                        match panel.handle_key(key) {
                            PanelEvent::Ignored => {
                                // Panel didn't handle it — move to left panel
                                self.focus = ConfigFocus::LeftPanel;
                            }
                            _ => {}
                        }
                    }
                    ConfigViewEvent::Consumed
                } else {
                    ConfigViewEvent::Ignored
                }
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if self.focus == ConfigFocus::LeftPanel {
                    return self.toggle_focus().unwrap_or(ConfigViewEvent::Consumed);
                } else if let Some(panel) = self.active_panel_mut() {
                    match panel.handle_key(key) {
                        PanelEvent::Ignored => ConfigViewEvent::Consumed, // already on last tab, stay
                        _ => ConfigViewEvent::Consumed,
                    }
                } else {
                    ConfigViewEvent::Consumed
                }
            }
            _ => ConfigViewEvent::Ignored,
        }
    }

    pub fn activated_name(&self) -> Option<&str> {
        self.activated_name.as_deref()
    }

    pub fn activated_config(&self) -> Option<&DeploymentConfig> {
        self.activated_config.as_ref()
    }

    /// Activate the currently selected deployment. Returns name + config for App.
    pub fn activate_selected(&mut self) -> Option<(&str, &DeploymentConfig)> {
        if let ActivePanel::Deployment(ref panel) = self.active_panel {
            self.activated_name = Some(panel.name().to_string());
            self.activated_config = Some(panel.config().clone());
            let name = self.activated_name.as_deref().unwrap();
            let config = self.activated_config.as_ref().unwrap();
            Some((name, config))
        } else {
            None
        }
    }

    /// Get the name of the currently selected item (deployment or hypervisor).
    pub fn selected_name(&self) -> Option<String> {
        match self.section {
            ConfigSection::Deployments => {
                self.deployment_list_state.selected()
                    .and_then(|idx| self.deployments.get(idx))
                    .cloned()
            }
            ConfigSection::Hypervisors => {
                self.hypervisor_list_state.selected()
                    .and_then(|idx| self.hypervisors.get(idx))
                    .cloned()
            }
        }
    }

    /// Deliver async data to the active panel.
    /// Returns an optional follow-up event (e.g. SSH valid → trigger Proxmox validation).
    pub fn deliver_data(&mut self, data: PanelData) -> Option<ConfigViewEvent> {
        // Check if this is an SSH probe result that should trigger Proxmox validation
        let trigger_proxmox = matches!(
            (&data, &self.active_panel),
            (PanelData::SshProbeResult(SshProbeStatus::Valid), ActivePanel::Hypervisor(_))
        );

        if let Some(panel) = self.active_panel_mut() {
            panel.receive_data(data);
        }

        if trigger_proxmox {
            if let ActivePanel::Hypervisor(panel) = &mut self.active_panel {
                if panel.proxmox_config().is_some() {
                    let host = panel.credentials_host().to_string();
                    let user = panel.credentials_user().to_string();
                    if !host.is_empty() {
                        panel.set_proxmox_checking();
                        return Some(ConfigViewEvent::ValidateProxmox { host, user });
                    }
                }
            }
        }
        None
    }

    /// Cancel any pending async fetch on the active panel.
    pub fn cancel_fetch(&mut self) {
        if let Some(panel) = self.active_panel_mut() {
            // Send an Esc key to cancel fetching state
            panel.handle_key(KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE));
        }
    }

    /// Get the Proxmox config from the active hypervisor panel (if any).
    pub fn active_hypervisor_proxmox_config(&self) -> Option<crate::config::types::ProxmoxHypervisorConfig> {
        if let ActivePanel::Hypervisor(panel) = &self.active_panel {
            panel.proxmox_config()
        } else {
            None
        }
    }

    /// Whether the active panel is capturing input (for status bar display).
    pub fn is_capturing(&self) -> bool {
        match &self.active_panel {
            ActivePanel::Deployment(p) => p.is_capturing(),
            ActivePanel::Hypervisor(p) => p.is_capturing(),
            ActivePanel::Prompt(_) => false,
        }
    }

    // --- Internal helpers ---

    fn active_panel_mut(&mut self) -> Option<&mut dyn ConfigPanel> {
        match &mut self.active_panel {
            ActivePanel::Deployment(p) => Some(p),
            ActivePanel::Hypervisor(p) => Some(p),
            ActivePanel::Prompt(_) => None,
        }
    }

    #[allow(dead_code)]
    fn active_panel_ref(&self) -> Option<&dyn ConfigPanel> {
        match &self.active_panel {
            ActivePanel::Deployment(p) => Some(p),
            ActivePanel::Hypervisor(p) => Some(p),
            ActivePanel::Prompt(_) => None,
        }
    }

    fn dispatch_panel_event(&mut self, event: PanelEvent) -> ConfigViewEvent {
        match event {
            PanelEvent::Consumed => ConfigViewEvent::Consumed,
            PanelEvent::Ignored => ConfigViewEvent::Consumed, // panel was capturing, still consume
            PanelEvent::Action(action) => match action {
                PanelAction::FetchGitRefs { repo_url } => {
                    ConfigViewEvent::FetchGitRefs { repo_url }
                }
                PanelAction::ProbeSsh { host, user } => {
                    ConfigViewEvent::ProbeSsh { host, user }
                }
                PanelAction::ValidateProxmox { host, user } => {
                    ConfigViewEvent::ValidateProxmox { host, user }
                }
                PanelAction::Deleted { ref name } => {
                    let name = name.clone();
                    self.handle_hypervisor_deleted(&name);
                    ConfigViewEvent::HypervisorDeleted { name }
                }
                PanelAction::Error(msg) => {
                    tracing::error!("{msg}");
                    ConfigViewEvent::Consumed
                }
            },
        }
    }

    fn handle_enter(&mut self, key: KeyEvent) -> ConfigViewEvent {
        if self.focus == ConfigFocus::LeftPanel {
            match self.section {
                ConfigSection::Deployments => {
                    if let Some(name) = self.selected_name() {
                        let already_active = self.activated_name.as_deref() == Some(&name);
                        if already_active {
                            return ConfigViewEvent::Consumed;
                        }
                        return ConfigViewEvent::RequestActivation { name };
                    }
                    ConfigViewEvent::Consumed
                }
                ConfigSection::Hypervisors => {
                    if self.is_add_entry_selected() {
                        self.start_create_flow();
                        ConfigViewEvent::Consumed
                    } else {
                        self.toggle_focus().unwrap_or(ConfigViewEvent::Consumed)
                    }
                }
            }
        } else {
            // Right panel — delegate to panel
            if let Some(panel) = self.active_panel_mut() {
                let event = panel.handle_key(key);
                self.dispatch_panel_event(event)
            } else {
                ConfigViewEvent::Consumed
            }
        }
    }

    /// Toggle focus between panels. Returns a ConfigViewEvent if the focus change
    /// triggers an async action (e.g. SSH probe when entering hypervisor panel).
    fn toggle_focus(&mut self) -> Option<ConfigViewEvent> {
        match self.focus {
            ConfigFocus::LeftPanel => {
                self.focus = ConfigFocus::RightPanel;
                self.request_panel_probe()
            }
            ConfigFocus::RightPanel => {
                self.focus = ConfigFocus::LeftPanel;
                None
            }
        }
    }

    /// Request SSH credential probes for the active panel.
    /// Returns the first probe as a ConfigViewEvent; spawns remaining probes
    /// via accumulated_probe_events (drained by handle_key callers).
    fn request_panel_probe(&mut self) -> Option<ConfigViewEvent> {
        match &mut self.active_panel {
            ActivePanel::Hypervisor(panel) => {
                if let Some(PanelAction::ProbeSsh { host, user }) = panel.request_probe() {
                    return Some(ConfigViewEvent::ProbeSsh { host, user });
                }
            }
            ActivePanel::Deployment(panel) => {
                // TODO: Currently only dispatches the first probe. When deployments
                // have multiple hosts, dispatch all probes (e.g. return a Vec or
                // accumulate additional events for App to drain).
                let actions = panel.request_probes();
                if let Some(PanelAction::ProbeSsh { host, user }) = actions.into_iter().next() {
                    return Some(ConfigViewEvent::ProbeSsh { host, user });
                }
            }
            _ => {}
        }
        None
    }

    // --- Left panel navigation ---

    /// Total items in hypervisors section (real hypervisors + "Add" entry).
    fn hypervisor_total(&self) -> usize {
        self.hypervisors.len() + 1
    }

    /// Whether the "Add hypervisor" virtual entry is currently selected.
    fn is_add_entry_selected(&self) -> bool {
        self.section == ConfigSection::Hypervisors
            && self.hypervisor_list_state.selected() == Some(self.hypervisors.len())
    }

    /// Load the appropriate panel for the current hypervisor list selection.
    fn load_hypervisor_for_selection(&mut self) {
        if self.is_add_entry_selected() {
            self.active_panel =
                ActivePanel::Prompt("Press Enter to create a new hypervisor.".into());
        } else {
            self.load_selected_hypervisor();
        }
    }

    fn left_panel_up(&mut self) {
        match self.section {
            ConfigSection::Deployments => {
                if let Some(idx) = self.deployment_list_state.selected() {
                    if idx > 0 {
                        self.deployment_list_state.select(Some(idx - 1));
                        self.load_selected_deployment();
                    } else if self.hypervisor_total() > 0 {
                        self.section = ConfigSection::Hypervisors;
                        self.hypervisor_list_state
                            .select(Some(self.hypervisor_total() - 1));
                        self.load_hypervisor_for_selection();
                    }
                }
            }
            ConfigSection::Hypervisors => {
                if let Some(idx) = self.hypervisor_list_state.selected() {
                    if idx > 0 {
                        self.hypervisor_list_state.select(Some(idx - 1));
                        self.load_hypervisor_for_selection();
                    } else if !self.deployments.is_empty() {
                        self.section = ConfigSection::Deployments;
                        self.deployment_list_state
                            .select(Some(self.deployments.len() - 1));
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
                    } else {
                        // Move to hypervisors section (always has at least the "Add" entry)
                        self.section = ConfigSection::Hypervisors;
                        self.hypervisor_list_state.select(Some(0));
                        self.load_hypervisor_for_selection();
                    }
                }
            }
            ConfigSection::Hypervisors => {
                if let Some(idx) = self.hypervisor_list_state.selected() {
                    if idx + 1 < self.hypervisor_total() {
                        self.hypervisor_list_state.select(Some(idx + 1));
                        self.load_hypervisor_for_selection();
                    } else if !self.deployments.is_empty() {
                        self.section = ConfigSection::Deployments;
                        self.deployment_list_state.select(Some(0));
                        self.load_selected_deployment();
                    }
                }
            }
        }
    }

    // --- Panel loading ---

    fn load_selected_deployment(&mut self) {
        if let Some(idx) = self.deployment_list_state.selected() {
            if let Some(name) = self.deployments.get(idx).cloned() {
                match load_deployment(&name) {
                    Ok(config) => {
                        let state = load_deployment_state(&name).unwrap_or_default();
                        let hyp = config
                            .deployment
                            .hypervisor
                            .as_ref()
                            .and_then(|href| load_hypervisor(&href.hypervisor_ref).ok());
                        self.active_panel =
                            ActivePanel::Deployment(DeploymentPanel::new(name, config, hyp, state));
                    }
                    Err(e) => {
                        self.active_panel =
                            ActivePanel::Prompt(format!("Error loading: {e}"));
                    }
                }
            }
        }
    }

    fn load_selected_hypervisor(&mut self) {
        if let Some(idx) = self.hypervisor_list_state.selected() {
            if let Some(name) = self.hypervisors.get(idx).cloned() {
                match load_hypervisor(&name) {
                    Ok(config) => {
                        let refs = find_referencing_deployments(&name).unwrap_or_default();
                        self.active_panel = ActivePanel::Hypervisor(
                            HypervisorPanel::new(name, config, refs),
                        );
                    }
                    Err(e) => {
                        self.active_panel =
                            ActivePanel::Prompt(format!("Error loading: {e}"));
                    }
                }
            }
        }
    }

    /// Handle HypervisorDeleted event — refresh list, select something else.
    fn handle_hypervisor_deleted(&mut self, _deleted_name: &str) {
        self.hypervisors = list_hypervisors().unwrap_or_default();
        if self.hypervisors.is_empty() {
            // No hypervisors left, switch to deployments
            self.section = ConfigSection::Deployments;
            if !self.deployments.is_empty() {
                self.deployment_list_state.select(Some(0));
                self.load_selected_deployment();
            } else {
                self.active_panel = ActivePanel::Prompt("No config items.".into());
            }
        } else {
            // Stay in hypervisors, select first
            self.hypervisor_list_state.select(Some(0));
            self.load_selected_hypervisor();
        }
    }

    // --- Create flow ---

    fn start_create_flow(&mut self) {
        let options = vec!["proxmox".to_string()]; // Only proxmox for now
        self.create_flow = CreateFlow::TypeSelection {
            picker: PopupPicker::new("Select hypervisor type", options),
        };
    }

    fn handle_create_flow_key(&mut self, key: KeyEvent) -> ConfigViewEvent {
        match &mut self.create_flow {
            CreateFlow::Inactive => ConfigViewEvent::Consumed,
            CreateFlow::TypeSelection { ref mut picker } => {
                match picker.handle_key(key) {
                    PopupAction::Continue => ConfigViewEvent::Consumed,
                    PopupAction::Cancel => {
                        self.create_flow = CreateFlow::Inactive;
                        ConfigViewEvent::Consumed
                    }
                    PopupAction::Selected(_, value) => {
                        let htype = match value.as_str() {
                            "proxmox" => HypervisorType::Proxmox,
                            _ => HypervisorType::Proxmox, // Default
                        };
                        self.create_flow = CreateFlow::NameInput {
                            htype,
                            input: Input::default(),
                        };
                        ConfigViewEvent::Consumed
                    }
                }
            }
            CreateFlow::NameInput {
                ref htype,
                ref mut input,
            } => match key.code {
                KeyCode::Enter => {
                    let name = input.value().trim().to_string();
                    let htype = htype.clone();
                    self.create_flow = CreateFlow::Inactive;

                    if name.is_empty() {
                        tracing::warn!("Hypervisor name cannot be empty");
                        return ConfigViewEvent::Consumed;
                    }
                    // Validate name: alphanumeric + hyphens
                    if !name
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                    {
                        tracing::warn!("Name can only contain letters, numbers, hyphens, underscores");
                        return ConfigViewEvent::Consumed;
                    }
                    if self.hypervisors.contains(&name) {
                        tracing::warn!("Hypervisor '{name}' already exists");
                        return ConfigViewEvent::Consumed;
                    }

                    match create_hypervisor(&name, htype) {
                        Ok(()) => {
                            tracing::info!("Created hypervisor '{name}'");
                            // Refresh list and select the new one
                            self.hypervisors = list_hypervisors().unwrap_or_default();
                            if let Some(idx) = self.hypervisors.iter().position(|n| n == &name) {
                                self.section = ConfigSection::Hypervisors;
                                self.hypervisor_list_state.select(Some(idx));
                                self.load_selected_hypervisor();
                            }
                        }
                        Err(e) => {
                            tracing::error!("Failed to create hypervisor: {e}");
                        }
                    }
                    ConfigViewEvent::Consumed
                }
                KeyCode::Esc => {
                    self.create_flow = CreateFlow::Inactive;
                    ConfigViewEvent::Consumed
                }
                _ => {
                    input.handle_event(&crossterm::event::Event::Key(key));
                    ConfigViewEvent::Consumed
                }
            },
        }
    }

    // --- Rendering ---

    fn render_impl(&self, frame: &mut Frame, area: Rect) {
        let p = Palette::default();

        let layout =
            Layout::horizontal([Constraint::Length(24), Constraint::Min(40)]).split(area);

        self.render_left_panel(frame, layout[0], &p);
        self.render_right_panel(frame, layout[1], &p);
    }

    fn render_left_panel(&self, frame: &mut Frame, area: Rect, p: &Palette) {
        let focused = self.focus == ConfigFocus::LeftPanel;
        let block = panel_block("Deployments", focused, p);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Always show the hypervisors section (we have the "Add" entry at minimum)
        let sections = Layout::vertical([
            Constraint::Min(4),
            Constraint::Length(1),
            Constraint::Min(3),
        ])
        .split(inner);

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
                    ListItem::new(name.as_str()).style(Style::default().fg(p.text_default))
                }
            })
            .collect();
        let deploy_list = List::new(items)
            .highlight_style(
                Style::default()
                    .fg(p.text_bright)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol(" > ")
            .highlight_spacing(ratatui::widgets::HighlightSpacing::Always);
        if self.section == ConfigSection::Deployments {
            let mut state = self.deployment_list_state.clone();
            frame.render_stateful_widget(deploy_list, sections[0], &mut state);
        } else {
            frame.render_widget(deploy_list, sections[0]);
        }

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " HYPERVISORS",
                Style::default()
                    .fg(p.text_bright)
                    .add_modifier(Modifier::BOLD),
            )))
            .style(Style::default().bg(p.bg_panel)),
            sections[1],
        );

        let mut hyp_items: Vec<ListItem> = self
            .hypervisors
            .iter()
            .map(|name| {
                ListItem::new(name.as_str()).style(Style::default().fg(p.text_tertiary))
            })
            .collect();
        // Virtual "Add hypervisor" entry
        hyp_items.push(
            ListItem::new(Line::from(Span::styled(
                "+ Add hypervisor",
                Style::default()
                    .fg(p.text_disabled)
                    .add_modifier(Modifier::ITALIC),
            ))),
        );
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

    fn render_right_panel(&self, frame: &mut Frame, area: Rect, p: &Palette) {
        let title = match &self.active_panel {
            ActivePanel::Deployment(panel) => panel.title(),
            ActivePanel::Hypervisor(panel) => panel.title(),
            ActivePanel::Prompt(_) => "Config".to_string(),
        };

        let focused = self.focus == ConfigFocus::RightPanel;
        let block = panel_block(&title, focused, p);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        match &self.active_panel {
            ActivePanel::Deployment(panel) => panel.render(frame, inner, p),
            ActivePanel::Hypervisor(panel) => panel.render(frame, inner, p),
            ActivePanel::Prompt(msg) => {
                frame.render_widget(
                    Paragraph::new(format!("  {msg}"))
                        .style(Style::default().fg(p.text_tertiary).bg(p.bg_panel)),
                    inner,
                );
            }
        }

        // Overlay: create flow
        match &self.create_flow {
            CreateFlow::Inactive => {}
            CreateFlow::TypeSelection { ref picker } => {
                picker.render(frame, inner, p);
            }
            CreateFlow::NameInput { htype, ref input } => {
                let type_str = format!("{htype:?}").to_lowercase();
                let title_text = format!("New {type_str} Hypervisor");

                let popup_h = 5u16.min(inner.height.saturating_sub(2)).max(3);
                let popup_w = 40u16.min(inner.width.saturating_sub(4));
                let x = inner.x + (inner.width.saturating_sub(popup_w)) / 2;
                let y = inner.y + (inner.height.saturating_sub(popup_h)) / 2;
                let popup_area = Rect::new(x, y, popup_w, popup_h);

                frame.render_widget(ratatui::widgets::Clear, popup_area);

                let block = ratatui::widgets::Block::default()
                    .title(format!(" {title_text} "))
                    .borders(ratatui::widgets::Borders::ALL)
                    .border_style(Style::default().fg(p.green_border))
                    .title_style(Style::default().fg(p.text_bright));
                let popup_inner = block.inner(popup_area);
                frame.render_widget(block, popup_area);

                if popup_inner.height > 0 {
                    let line = config_detail::render_input_line(input, " Name", p);
                    frame.render_widget(Paragraph::new(line), popup_inner);
                }
            }
        }
    }
}

impl Component for ConfigView {
    fn render(&self, frame: &mut Frame, area: Rect) {
        self.render_impl(frame, area);
    }
}
