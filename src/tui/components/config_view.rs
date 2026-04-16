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
use crate::config::loader::{
    find_referencing_deployments, list_deployments, list_hypervisors, load_deployment,
    load_deployment_state, load_hypervisor,
};
use crate::config::types::{DeploymentConfig, HypervisorType};
use crate::ssh::probe::SshProbeStatus;
use crate::tui::theme::{Palette, panel_block};

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
    ProbeSsh {
        host: String,
        user: String,
        port: u16,
    },
    /// Request Proxmox config validation.
    ValidateProxmox {
        host: String,
        user: String,
        port: u16,
    },
    /// Request ISO download.
    DownloadIso {
        host: String,
        user: String,
        port: u16,
        iso_storage_path: String,
        filename: String,
    },
    /// Request Proxmox VM list for deployment creation.
    ListProxmoxVms {
        name: String,
        host: String,
        user: String,
        port: u16,
        hypervisor_ref: String,
        import: bool,
    },
    /// Request Proxmox VM config query.
    QueryProxmoxVmConfig {
        name: String,
        host: String,
        user: String,
        port: u16,
        hypervisor_ref: String,
        vmid: u32,
    },
    /// Request host discovery for importing an existing deployment.
    DiscoverHost {
        name: String,
        host: String,
        user: String,
        port: u16,
        hypervisor_ref: Option<String>,
    },
    /// A hypervisor was deleted.
    HypervisorDeleted { name: String },
}

/// Right panel content — one variant per config type.
enum ActivePanel {
    Deployment(Box<DeploymentPanel>),
    Hypervisor(Box<HypervisorPanel>),
    Prompt(String),
}

/// State machine for the "Add" flows (hypervisor or deployment).
enum CreateFlow {
    Inactive,
    // --- Hypervisor flow ---
    /// User is picking the hypervisor type.
    HypervisorType {
        picker: PopupPicker,
    },
    /// User is entering the hypervisor name.
    HypervisorName {
        htype: HypervisorType,
        input: Input,
    },
    // --- Deployment flow ---
    /// User is entering the deployment name.
    DeploymentName {
        input: Input,
    },
    /// User is entering the host address.
    DeploymentHost {
        name: String,
        input: Input,
    },
    /// User is entering the SSH user.
    DeploymentSshUser {
        name: String,
        host: String,
        input: Input,
    },
    /// User is picking new vs existing.
    DeploymentMode {
        name: String,
        host: String,
        user: String,
        picker: PopupPicker,
    },
    /// User is picking a hypervisor (optional).
    DeploymentHypervisor {
        name: String,
        host: String,
        user: String,
        import: bool,
        picker: PopupPicker,
    },
    // --- Proxmox VM setup (after hypervisor selection) ---
    /// For new deployments: user enters vmid.
    ProxmoxVmId {
        name: String,
        host: String,
        user: String,
        hypervisor_ref: String,
        input: Input,
    },
    /// For existing deployments: user picks from VM list.
    ProxmoxVmPicker {
        name: String,
        host: String,
        user: String,
        hypervisor_ref: String,
        picker: PopupPicker,
        vms: Vec<crate::ops::hypervisor_proxmox_validate::ProxmoxVm>,
    },
    /// Waiting for async VM list or VM config query.
    ProxmoxVmLoading {
        message: String,
    },
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

    // Prerequisite check results
    prereqs: crate::ops::prereqs::PrereqResults,
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
            active_panel: ActivePanel::Prompt(
                "Select a deployment to view its configuration.".into(),
            ),
            create_flow: CreateFlow::Inactive,
            activated_name: Some(initial_deployment.to_string()),
            activated_config: None,
            prereqs: crate::ops::prereqs::PrereqResults::default(),
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
            KeyCode::Tab => self.toggle_focus().unwrap_or(ConfigViewEvent::Consumed),
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
                    if let Some(panel) = self.active_panel_mut()
                        && let PanelEvent::Ignored = panel.handle_key(key) {
                            // Panel didn't handle it — move to left panel
                            self.focus = ConfigFocus::LeftPanel;
                        }
                    ConfigViewEvent::Consumed
                } else {
                    ConfigViewEvent::Ignored
                }
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if self.focus == ConfigFocus::LeftPanel {
                    self.toggle_focus().unwrap_or(ConfigViewEvent::Consumed)
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

    #[allow(dead_code)]
    pub fn activated_name(&self) -> Option<&str> {
        self.activated_name.as_deref()
    }

    #[allow(dead_code)]
    pub fn activated_config(&self) -> Option<&DeploymentConfig> {
        self.activated_config.as_ref()
    }

    /// Activate the currently selected deployment. Returns name + config for App.
    pub fn activate_selected(&mut self) -> Option<(&str, &DeploymentConfig)> {
        if let ActivePanel::Deployment(ref panel) = self.active_panel {
            let name = panel.name().to_string();
            let config = panel.config().clone();
            self.activated_name = Some(name);
            self.activated_config = Some(config);
            Some((
                self.activated_name.as_deref()?,
                self.activated_config.as_ref()?,
            ))
        } else {
            None
        }
    }

    /// Get the name of the currently selected item (deployment or hypervisor).
    pub fn selected_name(&self) -> Option<String> {
        match self.section {
            ConfigSection::Deployments => self
                .deployment_list_state
                .selected()
                .and_then(|idx| self.deployments.get(idx))
                .cloned(),
            ConfigSection::Hypervisors => self
                .hypervisor_list_state
                .selected()
                .and_then(|idx| self.hypervisors.get(idx))
                .cloned(),
        }
    }

    /// Deliver async data to the active panel.
    /// Returns an optional follow-up event (e.g. SSH valid → trigger Proxmox validation).
    pub fn deliver_data(&mut self, data: PanelData) -> Option<ConfigViewEvent> {
        // Check if this is an SSH probe result that should trigger Proxmox validation
        let trigger_proxmox = matches!(
            (&data, &self.active_panel),
            (
                PanelData::SshProbeResult(SshProbeStatus::Valid),
                ActivePanel::Hypervisor(_)
            )
        );

        if let Some(panel) = self.active_panel_mut() {
            panel.receive_data(data);
        }

        if trigger_proxmox
            && let ActivePanel::Hypervisor(panel) = &mut self.active_panel
                && panel.proxmox_config().is_some() {
                    let host = panel.credentials_host().to_string();
                    let user = panel.credentials_user().to_string();
                    let port = panel.credentials_port();
                    if !host.is_empty() {
                        panel.set_proxmox_checking();
                        return Some(ConfigViewEvent::ValidateProxmox { host, user, port });
                    }
                }
        None
    }

    /// Cancel any pending async fetch on the active panel.
    pub fn cancel_fetch(&mut self) {
        if let Some(panel) = self.active_panel_mut() {
            // Send an Esc key to cancel fetching state
            panel.handle_key(KeyEvent::new(
                KeyCode::Esc,
                crossterm::event::KeyModifiers::NONE,
            ));
        }
    }

    /// Get the Proxmox config from the active hypervisor panel (if any).
    pub fn active_hypervisor_proxmox_config(
        &self,
    ) -> Option<crate::config::types::ProxmoxHypervisorConfig> {
        if let ActivePanel::Hypervisor(panel) = &self.active_panel {
            panel.proxmox_config()
        } else {
            None
        }
    }

    /// Get credentials from the active hypervisor panel (if any).
    pub fn active_hypervisor_credentials(&self) -> Option<(String, String, u16)> {
        if let ActivePanel::Hypervisor(panel) = &self.active_panel {
            let host = panel.credentials_host().to_string();
            let user = panel.credentials_user().to_string();
            let port = panel.credentials_port();
            if !host.is_empty() {
                return Some((host, user, port));
            }
        }
        None
    }

    /// Reload the currently selected deployment panel from disk.
    pub fn reload_active_deployment(&mut self) {
        if let Some(idx) = self.deployment_list_state.selected()
            && idx < self.deployments.len() {
                self.load_selected_deployment();
            }
    }

    /// Update prerequisite check results.
    pub fn set_prereqs(&mut self, results: crate::ops::prereqs::PrereqResults) {
        self.prereqs = results;
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
            ActivePanel::Deployment(p) => Some(p.as_mut()),
            ActivePanel::Hypervisor(p) => Some(p.as_mut()),
            ActivePanel::Prompt(_) => None,
        }
    }

    #[allow(dead_code)]
    fn active_panel_ref(&self) -> Option<&dyn ConfigPanel> {
        match &self.active_panel {
            ActivePanel::Deployment(p) => Some(p.as_ref()),
            ActivePanel::Hypervisor(p) => Some(p.as_ref()),
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
                PanelAction::ProbeSsh { host, user, port } => {
                    ConfigViewEvent::ProbeSsh { host, user, port }
                }
                PanelAction::ValidateProxmox { host, user, port } => {
                    ConfigViewEvent::ValidateProxmox { host, user, port }
                }
                PanelAction::DownloadIso {
                    host,
                    user,
                    port,
                    iso_storage_path,
                    filename,
                } => ConfigViewEvent::DownloadIso {
                    host,
                    user,
                    port,
                    iso_storage_path,
                    filename,
                },
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
                    if self.is_add_deployment_selected() {
                        self.start_deployment_create_flow();
                        return ConfigViewEvent::Consumed;
                    }
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
                    if self.is_add_hypervisor_selected() {
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
                if let Some(PanelAction::ProbeSsh { host, user, port }) = panel.request_probe() {
                    return Some(ConfigViewEvent::ProbeSsh { host, user, port });
                }
            }
            ActivePanel::Deployment(panel) => {
                // TODO: Currently only dispatches the first probe. When deployments
                // have multiple hosts, dispatch all probes (e.g. return a Vec or
                // accumulate additional events for App to drain).
                let actions = panel.request_probes();
                if let Some(PanelAction::ProbeSsh { host, user, port }) = actions.into_iter().next()
                {
                    return Some(ConfigViewEvent::ProbeSsh { host, user, port });
                }
            }
            _ => {}
        }
        None
    }

    // --- Left panel navigation ---

    /// Total items in hypervisors section (real hypervisors + "Add" entry).
    /// Total items in deployments section (real deployments + "Add" entry).
    fn deployment_total(&self) -> usize {
        self.deployments.len() + 1
    }

    fn hypervisor_total(&self) -> usize {
        self.hypervisors.len() + 1
    }

    /// Whether the "Add deployment" virtual entry is currently selected.
    fn is_add_deployment_selected(&self) -> bool {
        self.section == ConfigSection::Deployments
            && self.deployment_list_state.selected() == Some(self.deployments.len())
    }

    /// Whether the "Add hypervisor" virtual entry is currently selected.
    fn is_add_hypervisor_selected(&self) -> bool {
        self.section == ConfigSection::Hypervisors
            && self.hypervisor_list_state.selected() == Some(self.hypervisors.len())
    }

    /// Load the appropriate panel for the current deployment list selection.
    fn load_deployment_for_selection(&mut self) {
        if self.is_add_deployment_selected() {
            self.active_panel =
                ActivePanel::Prompt("Press Enter to create a new deployment.".into());
        } else {
            self.load_selected_deployment();
        }
    }

    /// Load the appropriate panel for the current hypervisor list selection.
    fn load_hypervisor_for_selection(&mut self) {
        if self.is_add_hypervisor_selected() {
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
                        self.load_deployment_for_selection();
                    } else {
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
                    } else {
                        self.section = ConfigSection::Deployments;
                        self.deployment_list_state
                            .select(Some(self.deployment_total() - 1));
                        self.load_deployment_for_selection();
                    }
                }
            }
        }
    }

    fn left_panel_down(&mut self) {
        match self.section {
            ConfigSection::Deployments => {
                if let Some(idx) = self.deployment_list_state.selected() {
                    if idx + 1 < self.deployment_total() {
                        self.deployment_list_state.select(Some(idx + 1));
                        self.load_deployment_for_selection();
                    } else {
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
        if let Some(idx) = self.deployment_list_state.selected()
            && let Some(name) = self.deployments.get(idx).cloned() {
                match load_deployment(&name) {
                    Ok(config) => {
                        let state = load_deployment_state(&name).unwrap_or_default();
                        let hyp = config
                            .deployment
                            .hypervisor
                            .as_ref()
                            .and_then(|href| load_hypervisor(&href.hypervisor_ref).ok());
                        self.active_panel =
                            ActivePanel::Deployment(Box::new(DeploymentPanel::new(name, config, hyp, state)));
                    }
                    Err(e) => {
                        self.active_panel = ActivePanel::Prompt(format!("Error loading: {e}"));
                    }
                }
            }
    }

    fn load_selected_hypervisor(&mut self) {
        if let Some(idx) = self.hypervisor_list_state.selected()
            && let Some(name) = self.hypervisors.get(idx).cloned() {
                match load_hypervisor(&name) {
                    Ok(config) => {
                        let refs = find_referencing_deployments(&name).unwrap_or_default();
                        self.active_panel =
                            ActivePanel::Hypervisor(Box::new(HypervisorPanel::new(name, config, refs)));
                    }
                    Err(e) => {
                        self.active_panel = ActivePanel::Prompt(format!("Error loading: {e}"));
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
        self.create_flow = CreateFlow::HypervisorType {
            picker: PopupPicker::new("Select hypervisor type", options),
        };
    }

    fn handle_create_flow_key(&mut self, key: KeyEvent) -> ConfigViewEvent {
        match &mut self.create_flow {
            CreateFlow::Inactive => ConfigViewEvent::Consumed,
            CreateFlow::HypervisorType { picker } => {
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
                        self.create_flow = CreateFlow::HypervisorName {
                            htype,
                            input: Input::default(),
                        };
                        ConfigViewEvent::Consumed
                    }
                }
            }
            CreateFlow::HypervisorName { htype, input } => match key.code {
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
                        tracing::warn!(
                            "Name can only contain letters, numbers, hyphens, underscores"
                        );
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
            // --- Deployment flow ---
            CreateFlow::DeploymentName { input } => match key.code {
                KeyCode::Enter => {
                    let name = input.value().trim().to_string();
                    if name.is_empty() {
                        tracing::warn!("Deployment name cannot be empty");
                        return ConfigViewEvent::Consumed;
                    }
                    if !name
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
                    {
                        tracing::warn!(
                            "Name can only contain letters, numbers, hyphens, underscores"
                        );
                        return ConfigViewEvent::Consumed;
                    }
                    if self.deployments.contains(&name) {
                        tracing::warn!("Deployment '{name}' already exists");
                        return ConfigViewEvent::Consumed;
                    }
                    self.create_flow = CreateFlow::DeploymentHost {
                        name,
                        input: Input::default(),
                    };
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
            CreateFlow::DeploymentHost { name, input } => match key.code {
                KeyCode::Enter => {
                    let host = input.value().trim().to_string();
                    if host.is_empty() {
                        tracing::warn!("Host address cannot be empty");
                        return ConfigViewEvent::Consumed;
                    }
                    let name = name.clone();
                    self.create_flow = CreateFlow::DeploymentSshUser {
                        name,
                        host,
                        input: Input::new(whoami::username()),
                    };
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
            CreateFlow::DeploymentSshUser { name, host, input } => match key.code {
                KeyCode::Enter => {
                    let user = input.value().trim().to_string();
                    if user.is_empty() {
                        tracing::warn!("SSH user cannot be empty");
                        return ConfigViewEvent::Consumed;
                    }
                    let name = name.clone();
                    let host = host.clone();
                    self.create_flow = CreateFlow::DeploymentMode {
                        name,
                        host,
                        user,
                        picker: PopupPicker::new(
                            "Deployment type",
                            vec![
                                "New (configure manually)".into(),
                                "Existing (import from host)".into(),
                            ],
                        ),
                    };
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
            CreateFlow::DeploymentMode {
                name,
                host,
                user,
                picker,
            } => {
                match picker.handle_key(key) {
                    PopupAction::Continue => ConfigViewEvent::Consumed,
                    PopupAction::Cancel => {
                        self.create_flow = CreateFlow::Inactive;
                        ConfigViewEvent::Consumed
                    }
                    PopupAction::Selected(idx, _) => {
                        let import = idx == 1;
                        let name = name.clone();
                        let host = host.clone();
                        let user = user.clone();
                        // Build hypervisor picker options
                        let mut options = vec!["None (bare metal)".to_string()];
                        options.extend(self.hypervisors.iter().cloned());
                        self.create_flow = CreateFlow::DeploymentHypervisor {
                            name,
                            host,
                            user,
                            import,
                            picker: PopupPicker::new("Select hypervisor (optional)", options),
                        };
                        ConfigViewEvent::Consumed
                    }
                }
            }
            CreateFlow::DeploymentHypervisor {
                name,
                host,
                user,
                import,
                picker,
            } => {
                match picker.handle_key(key) {
                    PopupAction::Continue => ConfigViewEvent::Consumed,
                    PopupAction::Cancel => {
                        self.create_flow = CreateFlow::Inactive;
                        ConfigViewEvent::Consumed
                    }
                    PopupAction::Selected(idx, value) => {
                        let import = *import;
                        let name = name.clone();
                        let host = host.clone();
                        let user = user.clone();

                        if idx == 0 {
                            // No hypervisor (bare metal)
                            self.create_flow = CreateFlow::Inactive;
                            if import {
                                self.create_new_deployment(&name, &host, &user, None);
                                self.active_panel = ActivePanel::Prompt(format!(
                                    "Discovering config from {user}@{host}..."
                                ));
                                return ConfigViewEvent::DiscoverHost {
                                    name,
                                    host,
                                    user,
                                    port: 22,
                                    hypervisor_ref: None,
                                };
                            }
                            self.create_new_deployment(&name, &host, &user, None);
                            return ConfigViewEvent::Consumed;
                        }

                        let hypervisor_ref = value;
                        // Check if this is a Proxmox hypervisor
                        let is_proxmox = load_hypervisor(&hypervisor_ref)
                            .map(|h| h.hypervisor.hypervisor_type == HypervisorType::Proxmox)
                            .unwrap_or(false);

                        if is_proxmox {
                            // Need VM setup — query Proxmox for VM list
                            self.create_flow = CreateFlow::ProxmoxVmLoading {
                                message: "Loading VMs from Proxmox...".into(),
                            };
                            return ConfigViewEvent::ListProxmoxVms {
                                name,
                                host,
                                user,
                                port: 22,
                                hypervisor_ref,
                                import,
                            };
                        }

                        // Non-Proxmox hypervisor — create directly
                        self.create_flow = CreateFlow::Inactive;
                        if import {
                            self.create_new_deployment(&name, &host, &user, Some(&hypervisor_ref));
                            self.active_panel = ActivePanel::Prompt(format!(
                                "Discovering config from {user}@{host}..."
                            ));
                            return ConfigViewEvent::DiscoverHost {
                                name,
                                host,
                                user,
                                port: 22,
                                hypervisor_ref: Some(hypervisor_ref),
                            };
                        }
                        self.create_new_deployment(&name, &host, &user, Some(&hypervisor_ref));
                        ConfigViewEvent::Consumed
                    }
                }
            }
            // --- Proxmox VM setup ---
            CreateFlow::ProxmoxVmLoading { .. } => {
                // Waiting for async VM list query — only Esc cancels
                if key.code == KeyCode::Esc {
                    self.create_flow = CreateFlow::Inactive;
                }
                ConfigViewEvent::Consumed
            }
            CreateFlow::ProxmoxVmId {
                name,
                host,
                user,
                hypervisor_ref,
                input,
            } => match key.code {
                KeyCode::Enter => {
                    let vmid_str = input.value().trim().to_string();
                    let name = name.clone();
                    let host = host.clone();
                    let user = user.clone();
                    let hypervisor_ref = hypervisor_ref.clone();
                    self.create_flow = CreateFlow::Inactive;

                    let vmid = match vmid_str.parse::<u32>() {
                        Ok(v) => v,
                        Err(_) => {
                            tracing::warn!("vmid must be a number");
                            return ConfigViewEvent::Consumed;
                        }
                    };

                    self.create_new_deployment_with_vm(
                        &name,
                        &host,
                        &user,
                        &hypervisor_ref,
                        crate::config::types::VmConfig {
                            vmid,
                            name: name.clone(),
                            ..Default::default()
                        },
                    );
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
            CreateFlow::ProxmoxVmPicker {
                name,
                host,
                user,
                hypervisor_ref,
                picker,
                vms,
            } => {
                match picker.handle_key(key) {
                    PopupAction::Continue => ConfigViewEvent::Consumed,
                    PopupAction::Cancel => {
                        self.create_flow = CreateFlow::Inactive;
                        ConfigViewEvent::Consumed
                    }
                    PopupAction::Selected(idx, _) => {
                        let name = name.clone();
                        let host = host.clone();
                        let user = user.clone();
                        let hypervisor_ref = hypervisor_ref.clone();
                        let vm = &vms[idx];
                        let vmid = vm.vmid;

                        // Query full VM config from Proxmox
                        self.create_flow = CreateFlow::ProxmoxVmLoading {
                            message: format!("Loading VM {} config...", vmid),
                        };
                        ConfigViewEvent::QueryProxmoxVmConfig {
                            name,
                            host,
                            user,
                            port: 22,
                            hypervisor_ref,
                            vmid,
                        }
                    }
                }
            }
        }
    }

    fn start_deployment_create_flow(&mut self) {
        self.create_flow = CreateFlow::DeploymentName {
            input: Input::default(),
        };
    }

    /// Create a new deployment with placeholder network config.
    pub fn create_new_deployment(
        &mut self,
        name: &str,
        host_address: &str,
        ssh_user: &str,
        hypervisor_ref: Option<&str>,
    ) {
        use std::collections::BTreeMap;

        let mut hosts = BTreeMap::new();
        hosts.insert(
            name.to_string(),
            crate::config::types::HostConfig {
                address: host_address.to_string(),
                ssh_user: ssh_user.to_string(),
                role: crate::config::types::HostRole::Combined,
                host_type: if hypervisor_ref.is_some() {
                    Some(crate::config::types::HostType::Vm)
                } else {
                    Some(crate::config::types::HostType::BareMetal)
                },
                ssh_port: None,
            },
        );

        let hypervisor = hypervisor_ref.map(|r| crate::config::types::HypervisorRef {
            hypervisor_ref: r.to_string(),
            vm: None,
        });

        let network = crate::config::types::NetworkConfig {
            gateway: "0.0.0.0".to_string(),
            external_dns_ips: vec![],
            internal_services_range: crate::config::types::IpRange {
                first: "0.0.0.0".to_string(),
                last: "0.0.0.0".to_string(),
            },
            infra_ip: "0.0.0.0".to_string(),
            instance_pool_range: crate::config::types::IpRange {
                first: "0.0.0.0".to_string(),
                last: "0.0.0.0".to_string(),
            },
            ntp_servers: None,
            dns_servers: None,
            external_dns_zone_name: None,
            rack_subnet: None,
            uplink_port_speed: None,
            allowed_source_ips: None,
        };

        let config = crate::config::types::DeploymentConfig {
            deployment: crate::config::types::DeploymentToml {
                deployment: crate::config::types::DeploymentMeta {
                    name: name.to_string(),
                    description: None,
                },
                hosts,
                network,
                nexus: crate::config::types::NexusConfig::default(),
                hypervisor,
            },
            build: default_build_config(),
            monitoring: crate::config::types::MonitoringToml::default(),
        };

        match crate::config::editor::create_deployment(name, &config) {
            Ok(()) => {
                tracing::info!("Created deployment '{name}'");
                self.deployments = list_deployments().unwrap_or_default();
                if let Some(idx) = self.deployments.iter().position(|n| n == name) {
                    self.section = ConfigSection::Deployments;
                    self.deployment_list_state.select(Some(idx));
                    self.load_selected_deployment();
                }
            }
            Err(e) => {
                tracing::error!("Failed to create deployment: {e}");
            }
        }
    }

    /// Create a new deployment with a VM config (Proxmox).
    fn create_new_deployment_with_vm(
        &mut self,
        name: &str,
        host_address: &str,
        ssh_user: &str,
        hypervisor_ref: &str,
        vm: crate::config::types::VmConfig,
    ) {
        use std::collections::BTreeMap;

        let mut hosts = BTreeMap::new();
        hosts.insert(
            name.to_string(),
            crate::config::types::HostConfig {
                address: host_address.to_string(),
                ssh_user: ssh_user.to_string(),
                role: crate::config::types::HostRole::Combined,
                host_type: Some(crate::config::types::HostType::Vm),
                ssh_port: None,
            },
        );

        let network = crate::config::types::NetworkConfig {
            gateway: "0.0.0.0".to_string(),
            external_dns_ips: vec![],
            internal_services_range: crate::config::types::IpRange {
                first: "0.0.0.0".to_string(),
                last: "0.0.0.0".to_string(),
            },
            infra_ip: "0.0.0.0".to_string(),
            instance_pool_range: crate::config::types::IpRange {
                first: "0.0.0.0".to_string(),
                last: "0.0.0.0".to_string(),
            },
            ntp_servers: None,
            dns_servers: None,
            external_dns_zone_name: None,
            rack_subnet: None,
            uplink_port_speed: None,
            allowed_source_ips: None,
        };

        let config = crate::config::types::DeploymentConfig {
            deployment: crate::config::types::DeploymentToml {
                deployment: crate::config::types::DeploymentMeta {
                    name: name.to_string(),
                    description: None,
                },
                hosts,
                network,
                nexus: crate::config::types::NexusConfig::default(),
                hypervisor: Some(crate::config::types::HypervisorRef {
                    hypervisor_ref: hypervisor_ref.to_string(),
                    vm: Some(vm),
                }),
            },
            build: default_build_config(),
            monitoring: crate::config::types::MonitoringToml::default(),
        };

        match crate::config::editor::create_deployment(name, &config) {
            Ok(()) => {
                tracing::info!("Created deployment '{name}' with Proxmox VM config");
                self.deployments = list_deployments().unwrap_or_default();
                if let Some(idx) = self.deployments.iter().position(|n| n == name) {
                    self.section = ConfigSection::Deployments;
                    self.deployment_list_state.select(Some(idx));
                    self.load_selected_deployment();
                }
            }
            Err(e) => {
                tracing::error!("Failed to create deployment: {e}");
            }
        }
    }

    /// Receive the VM list from Proxmox and show the appropriate UI.
    pub fn handle_proxmox_vm_list(
        &mut self,
        name: String,
        host: String,
        user: String,
        hypervisor_ref: String,
        import: bool,
        vms: Vec<crate::ops::hypervisor_proxmox_validate::ProxmoxVm>,
    ) {
        if import {
            // Existing deployment — show VM picker
            let options: Vec<String> = vms
                .iter()
                .map(|vm| format!("{} - {}", vm.vmid, vm.name))
                .collect();
            if options.is_empty() {
                tracing::warn!("No VMs found on Proxmox host");
                self.create_flow = CreateFlow::Inactive;
                return;
            }
            self.create_flow = CreateFlow::ProxmoxVmPicker {
                name,
                host,
                user,
                hypervisor_ref,
                picker: PopupPicker::new("Select VM", options),
                vms,
            };
        } else {
            // New deployment — prompt for vmid
            self.create_flow = CreateFlow::ProxmoxVmId {
                name,
                host,
                user,
                hypervisor_ref,
                input: Input::default(),
            };
        }
    }

    /// Receive VM config from Proxmox and create the deployment.
    pub fn handle_proxmox_vm_config(
        &mut self,
        name: String,
        host: String,
        user: String,
        hypervisor_ref: String,
        vm_config: crate::config::types::VmConfig,
    ) {
        self.create_flow = CreateFlow::Inactive;
        // Create with placeholders then trigger discovery for network/build
        self.create_new_deployment_with_vm(&name, &host, &user, &hypervisor_ref, vm_config);
        self.active_panel =
            ActivePanel::Prompt(format!("Discovering config from {user}@{host}..."));
    }

    /// Create a deployment from discovered config (import from existing host).
    pub fn create_discovered_deployment(
        &mut self,
        name: &str,
        host_address: &str,
        ssh_user: &str,
        hypervisor_ref: Option<&str>,
        discovered: &crate::ops::discover::DiscoveredConfig,
    ) {
        use std::collections::BTreeMap;

        let mut hosts = BTreeMap::new();
        hosts.insert(
            name.to_string(),
            crate::config::types::HostConfig {
                address: host_address.to_string(),
                ssh_user: ssh_user.to_string(),
                role: crate::config::types::HostRole::Combined,
                host_type: if hypervisor_ref.is_some() {
                    Some(crate::config::types::HostType::Vm)
                } else {
                    Some(crate::config::types::HostType::BareMetal)
                },
                ssh_port: None,
            },
        );

        // Preserve existing VM config if the deployment was already created
        // (e.g. by handle_proxmox_vm_config before discovery completed)
        let existing_vm = load_deployment(name)
            .ok()
            .and_then(|c| c.deployment.hypervisor)
            .and_then(|h| h.vm);

        let hypervisor = hypervisor_ref.map(|r| crate::config::types::HypervisorRef {
            hypervisor_ref: r.to_string(),
            vm: existing_vm,
        });

        let config = crate::config::types::DeploymentConfig {
            deployment: crate::config::types::DeploymentToml {
                deployment: crate::config::types::DeploymentMeta {
                    name: name.to_string(),
                    description: Some(format!("Imported from {host_address}")),
                },
                hosts,
                network: discovered.network.clone(),
                nexus: crate::config::types::NexusConfig::default(),
                hypervisor,
            },
            build: crate::config::types::BuildToml {
                omicron: crate::config::types::OmicronBuildConfig {
                    repo_path: discovered.omicron_path.clone(),
                    repo_url: None,
                    git_ref: None,
                    rust_toolchain: None,
                    overrides: discovered.overrides.clone(),
                },
                // TODO: Support multiple propolis patches in the future.
                // Currently hardcoded to the string-io-emulation patch.
                propolis: Some(crate::config::types::PropolisBuildConfig {
                    repo_path: "~/propolis".to_string(),
                    patched: Some(true),
                    patch_type: Some("string-io-emulation".to_string()),
                    source: Some(crate::config::types::PropolisSource::GithubRelease),
                    repo_url: Some("https://github.com/swherdman/propolis".to_string()),
                    git_ref: None,
                    local_binary: None,
                }),
                tuning: crate::config::types::TuningConfig::default(),
            },
            monitoring: crate::config::types::MonitoringToml::default(),
        };

        match crate::config::editor::create_deployment(name, &config) {
            Ok(()) => {
                tracing::info!("Created deployment '{name}' from discovered config");
                self.deployments = list_deployments().unwrap_or_default();
                if let Some(idx) = self.deployments.iter().position(|n| n == name) {
                    self.section = ConfigSection::Deployments;
                    self.deployment_list_state.select(Some(idx));
                    self.load_selected_deployment();
                }
            }
            Err(e) => {
                tracing::error!("Failed to create deployment: {e}");
            }
        }
    }

    // --- Rendering ---

    fn render_impl(&self, frame: &mut Frame, area: Rect) {
        let p = Palette::default();

        let layout = Layout::horizontal([Constraint::Length(24), Constraint::Min(40)]).split(area);

        self.render_left_panel(frame, layout[0], &p);
        self.render_right_panel(frame, layout[1], &p);
    }

    fn render_left_panel(&self, frame: &mut Frame, area: Rect, p: &Palette) {
        let focused = self.focus == ConfigFocus::LeftPanel;
        let block = panel_block("Deployments", focused, p);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Layout: deployments, HYPERVISORS header, hypervisors, PREREQUISITES section
        let sections = Layout::vertical([
            Constraint::Min(4),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(4), // PREREQUISITES header + docker + gh
        ])
        .split(inner);

        let mut items: Vec<ListItem> = self
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
        // Virtual "Add deployment" entry
        items.push(ListItem::new(Line::from(Span::styled(
            "+ Add deployment",
            Style::default()
                .fg(p.text_disabled)
                .add_modifier(Modifier::ITALIC),
        ))));
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
            .map(|name| ListItem::new(name.as_str()).style(Style::default().fg(p.text_tertiary)))
            .collect();
        // Virtual "Add hypervisor" entry
        hyp_items.push(ListItem::new(Line::from(Span::styled(
            "+ Add hypervisor",
            Style::default()
                .fg(p.text_disabled)
                .add_modifier(Modifier::ITALIC),
        ))));
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

        // Prerequisites section
        self.render_prereqs(frame, sections[3], p);
    }

    fn render_prereqs(&self, frame: &mut Frame, area: Rect, p: &Palette) {
        if area.height < 3 {
            return;
        }

        let chunks = Layout::vertical([
            Constraint::Length(1), // header
            Constraint::Length(1), // docker
            Constraint::Length(1), // gh
        ])
        .split(area);

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                " PREREQUISITES",
                Style::default()
                    .fg(p.text_bright)
                    .add_modifier(Modifier::BOLD),
            )))
            .style(Style::default().bg(p.bg_panel)),
            chunks[0],
        );

        let docker_line = prereq_line("docker", self.prereqs.docker, p);
        frame.render_widget(Paragraph::new(docker_line), chunks[1]);

        let gh_line = prereq_line("gh", self.prereqs.gh, p);
        frame.render_widget(Paragraph::new(gh_line), chunks[2]);
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
            CreateFlow::HypervisorType { picker }
            | CreateFlow::DeploymentMode { picker, .. }
            | CreateFlow::DeploymentHypervisor { picker, .. } => {
                picker.render(frame, inner, p);
            }
            CreateFlow::HypervisorName { htype, input } => {
                let type_str = format!("{htype:?}").to_lowercase();
                render_input_popup(
                    frame,
                    inner,
                    p,
                    &format!("New {type_str} Hypervisor"),
                    " Name",
                    input,
                );
            }
            CreateFlow::DeploymentName { input } => {
                render_input_popup(frame, inner, p, "New Deployment", " Name", input);
            }
            CreateFlow::DeploymentHost { name, input } => {
                render_input_popup(
                    frame,
                    inner,
                    p,
                    &format!("New Deployment: {name}"),
                    " Host",
                    input,
                );
            }
            CreateFlow::DeploymentSshUser { name, input, .. } => {
                render_input_popup(
                    frame,
                    inner,
                    p,
                    &format!("New Deployment: {name}"),
                    " SSH User",
                    input,
                );
            }
            CreateFlow::ProxmoxVmId { name, input, .. } => {
                render_input_popup(
                    frame,
                    inner,
                    p,
                    &format!("New Deployment: {name}"),
                    " VM ID",
                    input,
                );
            }
            CreateFlow::ProxmoxVmPicker { picker, .. } => {
                picker.render(frame, inner, p);
            }
            CreateFlow::ProxmoxVmLoading { message } => {
                let popup_h = inner.height.saturating_sub(2).clamp(3, 5);
                let popup_w = 40u16.min(inner.width.saturating_sub(4));
                let x = inner.x + (inner.width.saturating_sub(popup_w)) / 2;
                let y = inner.y + (inner.height.saturating_sub(popup_h)) / 2;
                let popup_area = Rect::new(x, y, popup_w, popup_h);
                frame.render_widget(ratatui::widgets::Clear, popup_area);
                frame.render_widget(
                    Paragraph::new(format!("  {message}"))
                        .style(Style::default().fg(p.yellow_warn).bg(p.bg_panel)),
                    popup_area,
                );
            }
        }
    }
}

impl Component for ConfigView {
    fn render(&self, frame: &mut Frame, area: Rect) {
        self.render_impl(frame, area);
    }
}

/// Default build config for new deployments — sets lab-appropriate overrides
/// that prevent Omicron from using production-sized defaults.
fn default_build_config() -> crate::config::types::BuildToml {
    crate::config::types::BuildToml {
        omicron: crate::config::types::OmicronBuildConfig {
            repo_path: "~/omicron".to_string(),
            repo_url: None,
            git_ref: None,
            rust_toolchain: None,
            overrides: crate::config::types::OmicronOverrides {
                cockroachdb_redundancy: Some(3),
                control_plane_storage_buffer_gib: Some(5),
                vdev_count: Some(3),
                vdev_size_bytes: Some(42949672960), // 40 GB
            },
        },
        propolis: None,
        tuning: crate::config::types::TuningConfig::default(),
    }
}

fn prereq_line<'a>(name: &str, status: crate::ops::prereqs::PrereqStatus, p: &Palette) -> Line<'a> {
    use crate::ops::prereqs::PrereqStatus;

    let (dot, color, label) = match status {
        PrereqStatus::Unknown => ("●", p.text_disabled, "checking..."),
        PrereqStatus::Ok => ("●", p.green_primary, ""),
        PrereqStatus::Degraded => ("●", p.yellow_warn, "not ready"),
        PrereqStatus::Missing => ("●", p.red_error, "not found"),
    };

    if label.is_empty() {
        Line::from(vec![
            Span::styled(format!("   {name} "), Style::default().fg(p.text_tertiary)),
            Span::styled(dot, Style::default().fg(color)),
        ])
    } else {
        Line::from(vec![
            Span::styled(format!("   {name} "), Style::default().fg(p.text_tertiary)),
            Span::styled(dot, Style::default().fg(color)),
            Span::styled(format!(" {label}"), Style::default().fg(color)),
        ])
    }
}

fn render_input_popup(
    frame: &mut Frame,
    area: Rect,
    p: &Palette,
    title: &str,
    label: &str,
    input: &Input,
) {
    let popup_h = 5u16.min(area.height.saturating_sub(2)).max(3);
    let popup_w = 45u16.min(area.width.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    let popup_area = Rect::new(x, y, popup_w, popup_h);

    frame.render_widget(ratatui::widgets::Clear, popup_area);

    let block = ratatui::widgets::Block::default()
        .title(format!(" {title} "))
        .borders(ratatui::widgets::Borders::ALL)
        .border_style(Style::default().fg(p.green_border))
        .title_style(Style::default().fg(p.text_bright));
    let popup_inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if popup_inner.height > 0 {
        let line = config_detail::render_input_line(input, label, p);
        frame.render_widget(Paragraph::new(line), popup_inner);
    }
}
