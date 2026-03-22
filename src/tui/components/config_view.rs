use std::cell::Cell;

use crossterm::event::Event as CtEvent;
use ratatui::prelude::*;
use ratatui::widgets::{List, ListItem, ListState, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::Component;
use super::git_ref_selector::{GitRefSelector, SelectorAction};
use crate::config::editor::update_deployment_field;
use crate::config::loader::{
    list_deployments, list_hypervisors, load_deployment, load_deployment_state, load_hypervisor,
};
use crate::config::types::{DeploymentConfig, DeploymentState, HypervisorConfig};
use crate::git::RepoRefs;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigTab {
    Host,
    Network,
    Build,
    Nexus,
    Monitoring,
}

impl ConfigTab {
    const ALL: [ConfigTab; 5] = [
        ConfigTab::Host,
        ConfigTab::Network,
        ConfigTab::Build,
        ConfigTab::Nexus,
        ConfigTab::Monitoring,
    ];

    fn label(self) -> &'static str {
        match self {
            ConfigTab::Host => "Host",
            ConfigTab::Network => "Network",
            ConfigTab::Build => "Build",
            ConfigTab::Nexus => "Nexus",
            ConfigTab::Monitoring => "Monitoring",
        }
    }
}

enum EditMode {
    Viewing,
    Editing { input: Input },
    /// Waiting for async fetch to complete.
    Fetching,
    /// Git ref selector popup.
    GitRefSelect { selector: GitRefSelector },
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
    /// If set, this field uses a picker instead of free-text editing.
    picker: Option<PickerKind>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerKind {
    GitRef { repo_url: String },
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

    // Per-tab detail lines
    tab_lines: [Vec<DetailLine>; 5],
    active_tab: ConfigTab,

    // The deployment that Build/Monitor/Recovery actually use
    activated_name: Option<String>,

    // Per-tab navigation state: (selected_line, scroll_offset)
    tab_nav: [(usize, usize); 5],
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
            tab_lines: [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            active_tab: ConfigTab::Host,
            activated_name: Some(initial_deployment.to_string()),
            tab_nav: [(0, 0); 5],
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

    /// Index of the active tab in the fixed-size arrays.
    fn tab_idx(&self) -> usize {
        self.active_tab as usize
    }

    /// Borrow the detail lines for the active tab.
    fn detail_lines(&self) -> &[DetailLine] {
        &self.tab_lines[self.tab_idx()]
    }

    /// Current selected line for the active tab.
    fn selected_line(&self) -> usize {
        self.tab_nav[self.tab_idx()].0
    }

    /// Current scroll offset for the active tab.
    fn scroll_offset(&self) -> usize {
        self.tab_nav[self.tab_idx()].1
    }

    fn set_selected_line(&mut self, v: usize) {
        self.tab_nav[self.tab_idx()].0 = v;
    }

    fn set_scroll_offset(&mut self, v: usize) {
        self.tab_nav[self.tab_idx()].1 = v;
    }

    /// Returns true if the active tab is the first one.
    pub fn is_first_tab(&self) -> bool {
        self.active_tab == ConfigTab::ALL[0]
    }

    /// Switch to the next tab (wrapping).
    pub fn next_tab(&mut self) {
        let idx = ConfigTab::ALL.iter().position(|t| *t == self.active_tab).unwrap_or(0);
        self.active_tab = ConfigTab::ALL[(idx + 1) % ConfigTab::ALL.len()];
    }

    /// Switch to the previous tab (wrapping).
    pub fn prev_tab(&mut self) {
        let idx = ConfigTab::ALL.iter().position(|t| *t == self.active_tab).unwrap_or(0);
        self.active_tab = ConfigTab::ALL[(idx + ConfigTab::ALL.len() - 1) % ConfigTab::ALL.len()];
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
                        self.tab_nav = [(0, 0); 5];
                        self.edit_mode = EditMode::Viewing;
                        self.rebuild_detail_lines();
                    }
                    Err(e) => {
                        self.active_config = None;
                        self.active_state = None;
                        self.active_hypervisor = None;
                        self.active_name = None;
                        self.tab_lines = [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()];
                        self.tab_lines[0] = vec![DetailLine {
                            text: format!("  Error loading: {e}"),
                            style: DetailStyle::Field,
                            field: None,
                            raw_value: None,
                            picker: None,
                        }];
                    }
                }
            }
        }
    }

    fn rebuild_detail_lines(&mut self) {
        let Some(config) = &self.active_config else {
            self.tab_lines = [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()];
            return;
        };

        let d = &config.deployment;
        let b = &config.build;

        // ── Host tab ──
        let mut host_tab = Vec::new();

        if let Some(desc) = &d.deployment.description {
            push_field(&mut host_tab, "description", desc);
        }

        for (name, host) in &d.hosts {
            push_header(&mut host_tab, name);
            push_editable(
                &mut host_tab,
                "address",
                &host.address,
                "deployment",
                &format!("hosts.{name}.address"),
            );
            push_editable(
                &mut host_tab,
                "ssh_user",
                &host.ssh_user,
                "deployment",
                &format!("hosts.{name}.ssh_user"),
            );
            push_field(
                &mut host_tab,
                "role",
                &format!("{:?}", host.role).to_lowercase(),
            );
            if let Some(ht) = &host.host_type {
                push_field(
                    &mut host_tab,
                    "type",
                    &format!("{ht:?}").to_lowercase(),
                );
            }
        }

        if let Some(href) = &d.hypervisor {
            if let Some(vm) = &href.vm {
                push_header(&mut host_tab, "VM");
                push_field(&mut host_tab, "hypervisor", &href.hypervisor_ref);
                push_editable(&mut host_tab, "vmid", &vm.vmid.to_string(), "deployment", "hypervisor.vm.vmid");
                push_editable(&mut host_tab, "name", &vm.name, "deployment", "hypervisor.vm.name");
                push_editable(
                    &mut host_tab,
                    "cores",
                    &vm.cores.to_string(),
                    "deployment",
                    "hypervisor.vm.cores",
                );
                push_editable(
                    &mut host_tab,
                    "sockets",
                    &vm.sockets.to_string(),
                    "deployment",
                    "hypervisor.vm.sockets",
                );
                push_editable(
                    &mut host_tab,
                    "memory_mb",
                    &vm.memory_mb.to_string(),
                    "deployment",
                    "hypervisor.vm.memory_mb",
                );
                push_editable(
                    &mut host_tab,
                    "disk_gb",
                    &vm.disk_gb.to_string(),
                    "deployment",
                    "hypervisor.vm.disk_gb",
                );
                push_editable(
                    &mut host_tab,
                    "disk_bus",
                    &vm.disk_bus,
                    "deployment",
                    "hypervisor.vm.disk_bus",
                );
                push_editable(
                    &mut host_tab,
                    "cpu_type",
                    &vm.cpu_type,
                    "deployment",
                    "hypervisor.vm.cpu_type",
                );
                push_editable(
                    &mut host_tab,
                    "net_model",
                    &vm.net_model,
                    "deployment",
                    "hypervisor.vm.net_model",
                );
                push_editable(
                    &mut host_tab,
                    "net_bridge",
                    &vm.net_bridge,
                    "deployment",
                    "hypervisor.vm.net_bridge",
                );
            }
        }

        // ── Network tab ──
        let mut network_tab = Vec::new();

        push_header(&mut network_tab, "ROUTING");
        push_editable(&mut network_tab, "gateway", &d.network.gateway, "deployment", "network.gateway");
        push_editable(&mut network_tab, "infra_ip", &d.network.infra_ip, "deployment", "network.infra_ip");
        push_editable(
            &mut network_tab,
            "rack_subnet",
            d.network.rack_subnet.as_deref().unwrap_or("fd00:1122:3344:0100::/56"),
            "deployment",
            "network.rack_subnet",
        );
        push_editable(
            &mut network_tab,
            "uplink_speed",
            d.network.uplink_port_speed.as_deref().unwrap_or("40G"),
            "deployment",
            "network.uplink_port_speed",
        );

        push_header(&mut network_tab, "DNS");
        push_editable(
            &mut network_tab,
            "external_dns_ips",
            &d.network.external_dns_ips.join(", "),
            "deployment",
            "network.external_dns_ips",
        );
        push_editable(
            &mut network_tab,
            "dns_zone_name",
            d.network.external_dns_zone_name.as_deref().unwrap_or("oxide.test"),
            "deployment",
            "network.external_dns_zone_name",
        );
        push_editable(
            &mut network_tab,
            "dns_resolvers",
            &d.network.dns_servers.as_ref().map(|v| v.join(", ")).unwrap_or_else(|| "1.1.1.1, 9.9.9.9".into()),
            "deployment",
            "network.dns_servers",
        );
        push_editable(
            &mut network_tab,
            "ntp_servers",
            &d.network.ntp_servers.as_ref().map(|v| v.join(", ")).unwrap_or_else(|| "0.pool.ntp.org".into()),
            "deployment",
            "network.ntp_servers",
        );

        push_header(&mut network_tab, "IP RANGES");
        push_editable(
            &mut network_tab,
            "services_first",
            &d.network.internal_services_range.first,
            "deployment",
            "network.internal_services_range.first",
        );
        push_editable(
            &mut network_tab,
            "services_last",
            &d.network.internal_services_range.last,
            "deployment",
            "network.internal_services_range.last",
        );
        push_editable(
            &mut network_tab,
            "pool_first",
            &d.network.instance_pool_range.first,
            "deployment",
            "network.instance_pool_range.first",
        );
        push_editable(
            &mut network_tab,
            "pool_last",
            &d.network.instance_pool_range.last,
            "deployment",
            "network.instance_pool_range.last",
        );

        push_header(&mut network_tab, "ACCESS");
        push_editable(
            &mut network_tab,
            "allowed_source_ips",
            d.network.allowed_source_ips.as_deref().unwrap_or("any"),
            "deployment",
            "network.allowed_source_ips",
        );

        // ── Build tab ──
        let mut build_tab = Vec::new();

        push_header(&mut build_tab, "OMICRON");
        push_editable(&mut build_tab, "repo_path", &b.omicron.repo_path, "build", "omicron.repo_path");
        push_editable(
            &mut build_tab,
            "repo_url",
            b.omicron.repo_url.as_deref().unwrap_or("https://github.com/oxidecomputer/omicron.git"),
            "build",
            "omicron.repo_url",
        );
        push_pickable(
            &mut build_tab,
            "git_ref",
            b.omicron.git_ref.as_deref().unwrap_or("HEAD"),
            "build",
            "omicron.git_ref",
            PickerKind::GitRef {
                repo_url: b.omicron.repo_url.clone()
                    .unwrap_or_else(|| "https://github.com/oxidecomputer/omicron.git".into()),
            },
        );
        push_editable(
            &mut build_tab,
            "rust_toolchain",
            b.omicron.rust_toolchain.as_deref().unwrap_or(""),
            "build",
            "omicron.rust_toolchain",
        );

        push_header(&mut build_tab, "OVERRIDES");
        push_editable(
            &mut build_tab,
            "crdb_redundancy",
            &b.omicron.overrides.cockroachdb_redundancy.map(|v| v.to_string()).unwrap_or_default(),
            "build",
            "omicron.overrides.cockroachdb_redundancy",
        );
        push_editable(
            &mut build_tab,
            "vdev_count",
            &b.omicron.overrides.vdev_count.map(|v| v.to_string()).unwrap_or_default(),
            "build",
            "omicron.overrides.vdev_count",
        );
        push_editable(
            &mut build_tab,
            "vdev_size_bytes",
            &b.omicron.overrides.vdev_size_bytes.map(|v| v.to_string()).unwrap_or_default(),
            "build",
            "omicron.overrides.vdev_size_bytes",
        );
        push_editable(
            &mut build_tab,
            "storage_buffer_gib",
            &b.omicron.overrides.control_plane_storage_buffer_gib.map(|v| v.to_string()).unwrap_or_default(),
            "build",
            "omicron.overrides.control_plane_storage_buffer_gib",
        );

        if let Some(propolis) = &b.propolis {
            push_header(&mut build_tab, "PROPOLIS");
            push_editable(
                &mut build_tab,
                "repo_path",
                &propolis.repo_path,
                "build",
                "propolis.repo_path",
            );
            push_field(
                &mut build_tab,
                "patched",
                &propolis.patched.map(|v| v.to_string()).unwrap_or_else(|| "(not set)".into()),
            );
            push_editable(
                &mut build_tab,
                "patch_type",
                propolis.patch_type.as_deref().unwrap_or(""),
                "build",
                "propolis.patch_type",
            );
            push_field(
                &mut build_tab,
                "source",
                &propolis.source.as_ref().map(|s| format!("{s:?}")).unwrap_or_else(|| "(not set)".into()),
            );
            push_editable(
                &mut build_tab,
                "repo_url",
                propolis.repo_url.as_deref().unwrap_or(""),
                "build",
                "propolis.repo_url",
            );
            if propolis.repo_url.is_some() {
                push_pickable(
                    &mut build_tab,
                    "git_ref",
                    propolis.git_ref.as_deref().unwrap_or("HEAD"),
                    "build",
                    "propolis.git_ref",
                    PickerKind::GitRef {
                        repo_url: propolis.repo_url.clone().unwrap_or_default(),
                    },
                );
            }
            push_editable(
                &mut build_tab,
                "local_binary",
                propolis.local_binary.as_deref().unwrap_or(""),
                "build",
                "propolis.local_binary",
            );
        }

        // ── Nexus tab ──
        let mut nexus_tab = Vec::new();
        let nx = &d.nexus;

        push_header(&mut nexus_tab, "SILO");
        push_editable(&mut nexus_tab, "silo_name", &nx.silo_name, "deployment", "nexus.silo_name");
        push_editable(&mut nexus_tab, "username", &nx.username, "deployment", "nexus.username");
        push_editable(&mut nexus_tab, "password", &nx.password, "deployment", "nexus.password");

        push_header(&mut nexus_tab, "IP POOL");
        push_editable(&mut nexus_tab, "pool_name", &nx.ip_pool_name, "deployment", "nexus.ip_pool_name");

        push_header(&mut nexus_tab, "QUOTAS");
        push_editable(
            &mut nexus_tab,
            "cpus",
            &nx.quotas.cpus.to_string(),
            "deployment",
            "nexus.quotas.cpus",
        );
        push_editable(
            &mut nexus_tab,
            "memory",
            &nx.quotas.memory.to_string(),
            "deployment",
            "nexus.quotas.memory",
        );
        push_editable(
            &mut nexus_tab,
            "storage",
            &nx.quotas.storage.to_string(),
            "deployment",
            "nexus.quotas.storage",
        );

        // ── Monitoring tab ──
        let mut monitoring_tab = Vec::new();
        let mon = &config.monitoring;

        push_header(&mut monitoring_tab, "THRESHOLDS");
        push_editable(
            &mut monitoring_tab,
            "rpool_warning_%",
            &mon.thresholds.rpool_warning_percent.to_string(),
            "monitoring",
            "thresholds.rpool_warning_percent",
        );
        push_editable(
            &mut monitoring_tab,
            "rpool_critical_%",
            &mon.thresholds.rpool_critical_percent.to_string(),
            "monitoring",
            "thresholds.rpool_critical_percent",
        );
        push_editable(
            &mut monitoring_tab,
            "vdev_warning_gib",
            &mon.thresholds.vdev_warning_gib.to_string(),
            "monitoring",
            "thresholds.vdev_warning_gib",
        );
        push_editable(
            &mut monitoring_tab,
            "oxp_pool_warning_%",
            &mon.thresholds.oxp_pool_warning_percent.to_string(),
            "monitoring",
            "thresholds.oxp_pool_warning_percent",
        );

        push_header(&mut monitoring_tab, "POLLING");
        push_editable(
            &mut monitoring_tab,
            "status_interval_secs",
            &mon.polling.status_interval_secs.to_string(),
            "monitoring",
            "polling.status_interval_secs",
        );
        push_editable(
            &mut monitoring_tab,
            "disk_interval_secs",
            &mon.polling.disk_interval_secs.to_string(),
            "monitoring",
            "polling.disk_interval_secs",
        );

        // Drift info on monitoring tab
        if let Some(state) = &self.active_state {
            if let Some(drift) = &state.drift {
                push_header(&mut monitoring_tab, "DRIFT");
                push_field(&mut monitoring_tab, "last_checked", &drift.last_checked);
            }
        }

        // Tuning fields appended to build tab
        let t = &b.tuning;

        push_header(&mut build_tab, "TUNING");
        push_editable(
            &mut build_tab,
            "svcadm_autoclear",
            &t.svcadm_autoclear.map(|v| v.to_string()).unwrap_or_else(|| "false".into()),
            "build",
            "tuning.svcadm_autoclear",
        );
        push_editable(
            &mut build_tab,
            "swap_size_gb",
            &t.swap_size_gb.map(|v| v.to_string()).unwrap_or_else(|| "8".into()),
            "build",
            "tuning.swap_size_gb",
        );
        push_editable(
            &mut build_tab,
            "vdev_dir",
            t.vdev_dir.as_deref().unwrap_or("/var/tmp"),
            "build",
            "tuning.vdev_dir",
        );
        push_editable(
            &mut build_tab,
            "memory_earmark_mb",
            &t.memory_earmark_mb.map(|v| v.to_string()).unwrap_or_else(|| "6144".into()),
            "build",
            "tuning.memory_earmark_mb",
        );
        push_editable(
            &mut build_tab,
            "vmm_reservoir_%",
            &t.vmm_reservoir_percentage.map(|v| v.to_string()).unwrap_or_else(|| "60".into()),
            "build",
            "tuning.vmm_reservoir_percentage",
        );
        push_editable(
            &mut build_tab,
            "swap_device_size_gb",
            &t.swap_device_size_gb.map(|v| v.to_string()).unwrap_or_else(|| "64".into()),
            "build",
            "tuning.swap_device_size_gb",
        );

        self.tab_lines = [host_tab, network_tab, build_tab, nexus_tab, monitoring_tab];
    }

    // --- Navigation ---

    pub fn navigate_up(&mut self) {
        match self.focus {
            ConfigFocus::LeftPanel => self.left_panel_up(),
            ConfigFocus::RightPanel => {
                let sel = self.selected_line();
                if let Some(prev) = self.prev_editable_line(sel) {
                    self.set_selected_line(prev);
                    self.ensure_visible();
                }
            }
        }
    }

    pub fn navigate_down(&mut self) {
        match self.focus {
            ConfigFocus::LeftPanel => self.left_panel_down(),
            ConfigFocus::RightPanel => {
                let sel = self.selected_line();
                if let Some(next) = self.next_editable_line(sel) {
                    self.set_selected_line(next);
                    self.ensure_visible();
                }
            }
        }
    }

    fn next_editable_line(&self, from: usize) -> Option<usize> {
        let lines = self.detail_lines();
        (from + 1..lines.len())
            .find(|&i| lines[i].field.is_some())
    }

    fn prev_editable_line(&self, from: usize) -> Option<usize> {
        let lines = self.detail_lines();
        (0..from)
            .rev()
            .find(|&i| lines[i].field.is_some())
    }

    fn first_editable_line(&self) -> usize {
        self.detail_lines()
            .iter()
            .position(|l| l.field.is_some())
            .unwrap_or(0)
    }

    fn ensure_visible(&mut self) {
        let sel = self.selected_line();
        let off = self.scroll_offset();
        if sel < off {
            self.set_scroll_offset(sel);
        }
        let h = self.visible_height.get();
        if h > 0 && sel >= off + h {
            self.set_scroll_offset(sel - h + 1);
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
                let first = self.first_editable_line();
                self.set_selected_line(first);
                self.set_scroll_offset(0);
                ConfigFocus::RightPanel
            }
            ConfigFocus::RightPanel => ConfigFocus::LeftPanel,
        };
    }

    // --- Editing ---

    pub fn start_edit(&mut self) {
        let sel = self.selected_line();
        if let Some(line) = self.detail_lines().get(sel) {
            if line.field.is_some() {
                if line.picker.is_some() {
                    // Picker field — enter fetching state; caller triggers async fetch
                    self.edit_mode = EditMode::Fetching;
                } else {
                    let value = line.raw_value.clone().unwrap_or_default();
                    let input = Input::new(value);
                    self.edit_mode = EditMode::Editing { input };
                }
            }
        }
    }

    /// Returns the picker kind for the currently selected field, if any.
    pub fn pending_picker(&self) -> Option<PickerKind> {
        if !matches!(self.edit_mode, EditMode::Fetching) {
            return None;
        }
        let sel = self.selected_line();
        self.detail_lines().get(sel).and_then(|l| l.picker.clone())
    }

    /// Open the git ref selector with fetched cache data.
    pub fn open_git_ref_selector(&mut self, cache: &RepoRefs) {
        let current = self.detail_lines().get(self.selected_line())
            .and_then(|l| l.raw_value.as_deref())
            .and_then(|v| if v == "HEAD" { None } else { Some(v) });
        let selector = GitRefSelector::new(current, cache.clone());
        self.edit_mode = EditMode::GitRefSelect { selector };
    }

    /// Handle a key event when the git ref selector or fetching is active.
    /// Returns None if the key was consumed, or Some(result) if editing completed.
    pub fn handle_git_ref_key(&mut self, key: crossterm::event::KeyEvent) -> Option<Result<(), String>> {
        // During Fetching, only Esc cancels
        if matches!(self.edit_mode, EditMode::Fetching) {
            if key.code == crossterm::event::KeyCode::Esc {
                self.edit_mode = EditMode::Viewing;
            }
            return None;
        }

        let EditMode::GitRefSelect { ref mut selector } = self.edit_mode else {
            return None;
        };
        match selector.handle_key(key) {
            SelectorAction::Continue => None,
            SelectorAction::Cancel => {
                self.edit_mode = EditMode::Viewing;
                None
            }
            SelectorAction::Confirm(value) => {
                self.edit_mode = EditMode::Viewing;
                Some(self.persist_field(&value))
            }
        }
    }

    pub fn cancel_edit(&mut self) {
        self.edit_mode = EditMode::Viewing;
    }

    /// Returns true when we're in the git ref selector or fetching state.
    pub fn is_picking(&self) -> bool {
        matches!(self.edit_mode, EditMode::GitRefSelect { .. } | EditMode::Fetching)
    }

    /// Confirm the edit, persist to disk, reload.
    pub fn confirm_edit(&mut self) -> Result<(), String> {
        let EditMode::Editing { ref input } = self.edit_mode else {
            return Ok(());
        };
        let new_value = input.value().to_string();
        self.edit_mode = EditMode::Viewing;
        self.persist_field(&new_value)
    }

    fn persist_field(&mut self, new_value: &str) -> Result<(), String> {
        let Some(name) = &self.active_name else { return Ok(()) };
        let sel = self.selected_line();
        let Some(line) = self.detail_lines().get(sel) else { return Ok(()) };
        let Some(field_key) = &line.field else { return Ok(()) };

        let name = name.clone();
        let file = field_key.file;
        let path = field_key.path.clone();

        // "HEAD" means use default (latest) — store as empty to clear the override
        let save_value = if new_value == "HEAD" { "" } else { new_value };

        if let Err(e) = update_deployment_field(&name, file, &path, save_value) {
            let msg = format!("Failed to save {path}: {e}");
            tracing::error!("{msg}");
            self.load_selected_deployment();
            return Err(msg);
        }

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

        // Split: tab bar (1 line) + content
        let chunks = Layout::vertical([Constraint::Length(1), Constraint::Min(1)])
            .split(inner);

        // ── Tab bar ──
        self.render_tab_row(frame, chunks[0], p);

        let content_area = chunks[1];
        let detail_lines = self.detail_lines();

        if detail_lines.is_empty() && self.active_config.is_none() {
            frame.render_widget(
                Paragraph::new("  Select a deployment to view its configuration.")
                    .style(Style::default().fg(p.text_tertiary).bg(p.bg_panel)),
                content_area,
            );
            return;
        }

        // Store visible height for scroll calculations in ensure_visible()
        self.visible_height.set(content_area.height as usize);

        let sel = self.selected_line();
        let scroll_off = self.scroll_offset();

        let lines: Vec<Line> = detail_lines
            .iter()
            .enumerate()
            .map(|(i, dl)| {
                let is_selected = i == sel && self.focus == ConfigFocus::RightPanel;
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
        let visible_height = content_area.height as usize;
        frame.render_widget(
            Paragraph::new(lines)
                .style(Style::default().bg(p.bg_panel))
                .scroll((scroll_off as u16, 0)),
            content_area,
        );

        if total_lines > visible_height {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(p.border_default));
            let mut scrollbar_state = ScrollbarState::new(total_lines.saturating_sub(visible_height))
                .position(scroll_off);
            frame.render_stateful_widget(scrollbar, content_area, &mut scrollbar_state);
        }

        // Overlay for fetching / git ref selector
        match &self.edit_mode {
            EditMode::Fetching => {
                let popup_area = Rect::new(
                    content_area.x + 2,
                    content_area.y + 1,
                    content_area.width.saturating_sub(4).min(40),
                    1,
                );
                frame.render_widget(ratatui::widgets::Clear, popup_area);
                frame.render_widget(
                    Paragraph::new("  Fetching refs...")
                        .style(Style::default().fg(p.yellow_warn).bg(p.bg_panel)),
                    popup_area,
                );
            }
            EditMode::GitRefSelect { ref selector } => {
                selector.render(frame, content_area, p);
            }
            _ => {}
        }
    }

    fn render_tab_row(&self, frame: &mut Frame, area: Rect, p: &Palette) {
        let mut spans: Vec<Span> = Vec::new();

        for (i, tab) in ConfigTab::ALL.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" │ ", Style::default().fg(p.border_default)));
            }
            if *tab == self.active_tab {
                spans.push(Span::styled(
                    tab.label(),
                    Style::default()
                        .fg(p.green_primary)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(
                    tab.label(),
                    Style::default().fg(p.text_disabled),
                ));
            }
        }

        frame.render_widget(
            Paragraph::new(Line::from(spans)).style(Style::default().bg(p.bg_panel)),
            area,
        );
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
        picker: None,
    });
    lines.push(DetailLine {
        text: format!("  {title}"),
        style: DetailStyle::SectionHeader,
        field: None,
        raw_value: None,
        picker: None,
    });
}

fn push_field(lines: &mut Vec<DetailLine>, label: &str, value: &str) {
    lines.push(DetailLine {
        text: format!("    {label}: {value}"),
        style: DetailStyle::Field,
        field: None,
        raw_value: None,
        picker: None,
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
        picker: None,
    });
}

fn push_pickable(
    lines: &mut Vec<DetailLine>,
    label: &str,
    value: &str,
    file: &'static str,
    path: &str,
    kind: PickerKind,
) {
    lines.push(DetailLine {
        text: format!("    {label}: {value}"),
        style: DetailStyle::EditableField,
        field: Some(FieldKey {
            file,
            path: path.to_string(),
        }),
        raw_value: Some(value.to_string()),
        picker: Some(kind),
    });
}
