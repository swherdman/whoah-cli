use std::cell::Cell;

use crossterm::event::{Event as CtEvent, KeyCode, KeyEvent};
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Tabs};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::config_detail::{
    self, ConfigPanel, DetailLine, PickerKind, PanelAction, PanelData, PanelEvent,
    push_editable, push_field, push_header, push_pickable, render_detail_lines,
};
use super::git_ref_selector::{GitRefSelector, SelectorAction};
use crate::config::editor::update_deployment_field;
use crate::config::loader::{load_deployment, load_deployment_state, load_hypervisor};
use crate::config::types::{DeploymentConfig, DeploymentState, HypervisorConfig};
use crate::tui::theme::Palette;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigTab {
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
    /// Waiting for async git ref fetch.
    Fetching,
    /// Git ref selector popup.
    GitRefSelect { selector: GitRefSelector },
}

pub struct DeploymentPanel {
    name: String,
    config: DeploymentConfig,
    hypervisor: Option<HypervisorConfig>,
    state: DeploymentState,

    tab_lines: [Vec<DetailLine>; 5],
    active_tab: ConfigTab,
    tab_nav: [(usize, usize); 5],
    edit_mode: EditMode,
    visible_height: Cell<usize>,
}

impl DeploymentPanel {
    pub fn new(
        name: String,
        config: DeploymentConfig,
        hypervisor: Option<HypervisorConfig>,
        state: DeploymentState,
    ) -> Self {
        let mut panel = Self {
            name,
            config,
            hypervisor,
            state,
            tab_lines: [Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new()],
            active_tab: ConfigTab::Host,
            tab_nav: [(0, 0); 5],
            edit_mode: EditMode::Viewing,
            visible_height: Cell::new(0),
        };
        panel.rebuild_detail_lines();
        panel
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn config(&self) -> &DeploymentConfig {
        &self.config
    }

    // --- Tab/line indexing ---

    fn tab_idx(&self) -> usize {
        self.active_tab as usize
    }

    fn detail_lines(&self) -> &[DetailLine] {
        &self.tab_lines[self.tab_idx()]
    }

    fn selected_line(&self) -> usize {
        self.tab_nav[self.tab_idx()].0
    }

    fn scroll_offset(&self) -> usize {
        self.tab_nav[self.tab_idx()].1
    }

    fn set_selected_line(&mut self, v: usize) {
        self.tab_nav[self.tab_idx()].0 = v;
    }

    fn set_scroll_offset(&mut self, v: usize) {
        self.tab_nav[self.tab_idx()].1 = v;
    }

    fn is_first_tab(&self) -> bool {
        self.active_tab == ConfigTab::ALL[0]
    }

    fn is_last_tab(&self) -> bool {
        self.active_tab == *ConfigTab::ALL.last().unwrap()
    }

    fn next_tab(&mut self) {
        let idx = ConfigTab::ALL
            .iter()
            .position(|t| *t == self.active_tab)
            .unwrap_or(0);
        self.active_tab = ConfigTab::ALL[(idx + 1) % ConfigTab::ALL.len()];
    }

    fn prev_tab(&mut self) {
        let idx = ConfigTab::ALL
            .iter()
            .position(|t| *t == self.active_tab)
            .unwrap_or(0);
        self.active_tab =
            ConfigTab::ALL[(idx + ConfigTab::ALL.len() - 1) % ConfigTab::ALL.len()];
    }

    // --- Navigation ---

    fn navigate_up(&mut self) {
        let sel = self.selected_line();
        if let Some(prev) = config_detail::prev_editable_line(self.detail_lines(), sel) {
            self.set_selected_line(prev);
            self.ensure_visible();
        }
    }

    fn navigate_down(&mut self) {
        let sel = self.selected_line();
        if let Some(next) = config_detail::next_editable_line(self.detail_lines(), sel) {
            self.set_selected_line(next);
            self.ensure_visible();
        }
    }

    fn ensure_visible(&mut self) {
        let sel = self.selected_line();
        let mut off = self.scroll_offset();
        let h = self.visible_height.get();
        config_detail::ensure_visible(sel, &mut off, h);
        self.set_scroll_offset(off);
    }

    fn focus_first_editable(&mut self) {
        let first = config_detail::first_editable_line(self.detail_lines());
        self.set_selected_line(first);
        self.set_scroll_offset(0);
    }

    // --- Editing ---

    fn start_edit(&mut self) -> Option<PanelAction> {
        let sel = self.selected_line();
        let lines = self.detail_lines();
        let Some(line) = lines.get(sel) else {
            return None;
        };
        if line.field.is_none() {
            return None;
        }

        if let Some(picker) = &line.picker {
            let action = match picker {
                PickerKind::GitRef { ref repo_url } => {
                    Some(PanelAction::FetchGitRefs {
                        repo_url: repo_url.clone(),
                    })
                }
            };
            self.edit_mode = EditMode::Fetching;
            return action;
        }

        let value = line.raw_value.clone().unwrap_or_default();
        self.edit_mode = EditMode::Editing { input: Input::new(value) };
        None
    }

    fn confirm_edit(&mut self) -> Result<(), String> {
        let EditMode::Editing { ref input } = self.edit_mode else {
            return Ok(());
        };
        let new_value = input.value().to_string();
        self.edit_mode = EditMode::Viewing;
        self.persist_field(&new_value)
    }

    fn cancel_edit(&mut self) {
        self.edit_mode = EditMode::Viewing;
    }

    fn persist_field(&mut self, new_value: &str) -> Result<(), String> {
        let sel = self.selected_line();
        let Some(line) = self.detail_lines().get(sel) else {
            return Ok(());
        };
        let Some(field_key) = &line.field else {
            return Ok(());
        };

        let file = field_key.file;
        let path = field_key.path.clone();

        // "HEAD" means use default (latest) — store as empty to clear the override
        let save_value = if new_value == "HEAD" { "" } else { new_value };

        if let Err(e) = update_deployment_field(&self.name, file, &path, save_value) {
            let msg = format!("Failed to save {path}: {e}");
            tracing::error!("{msg}");
            self.reload();
            return Err(msg);
        }

        self.reload();
        Ok(())
    }

    /// Reload config from disk and rebuild detail lines.
    fn reload(&mut self) {
        if let Ok(config) = load_deployment(&self.name) {
            let state = load_deployment_state(&self.name).unwrap_or_default();
            let hyp = config
                .deployment
                .hypervisor
                .as_ref()
                .and_then(|href| load_hypervisor(&href.hypervisor_ref).ok());
            self.config = config;
            self.state = state;
            self.hypervisor = hyp;
            self.rebuild_detail_lines();
        }
    }

    fn handle_git_ref_key(&mut self, key: KeyEvent) -> PanelEvent {
        // During Fetching, only Esc cancels
        if matches!(self.edit_mode, EditMode::Fetching) {
            if key.code == KeyCode::Esc {
                self.edit_mode = EditMode::Viewing;
            }
            return PanelEvent::Consumed;
        }

        let EditMode::GitRefSelect { ref mut selector } = self.edit_mode else {
            return PanelEvent::Consumed;
        };
        match selector.handle_key(key) {
            SelectorAction::Continue => PanelEvent::Consumed,
            SelectorAction::Cancel => {
                self.edit_mode = EditMode::Viewing;
                PanelEvent::Consumed
            }
            SelectorAction::Confirm(value) => {
                self.edit_mode = EditMode::Viewing;
                if let Err(msg) = self.persist_field(&value) {
                    PanelEvent::Action(PanelAction::Error(msg))
                } else {
                    PanelEvent::Consumed
                }
            }
        }
    }

    fn handle_edit_event(&mut self, key: KeyEvent) {
        if let EditMode::Editing { ref mut input } = self.edit_mode {
            input.handle_event(&CtEvent::Key(key));
        }
    }

    // --- Rendering ---

    fn render_tab_row(&self, frame: &mut Frame, area: Rect, p: &Palette) {
        let titles: Vec<&str> = ConfigTab::ALL.iter().map(|t| t.label()).collect();
        let tabs = Tabs::new(titles)
            .select(self.tab_idx())
            .style(Style::default().fg(p.text_disabled).bg(p.bg_panel))
            .highlight_style(
                Style::default()
                    .fg(p.green_primary)
                    .add_modifier(Modifier::BOLD),
            )
            .divider("│");
        frame.render_widget(tabs, area);
    }

    // --- Detail line building ---

    fn rebuild_detail_lines(&mut self) {
        let d = &self.config.deployment;
        let b = &self.config.build;

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
                push_field(&mut host_tab, "type", &format!("{ht:?}").to_lowercase());
            }
        }

        if let Some(href) = &d.hypervisor {
            if let Some(vm) = &href.vm {
                push_header(&mut host_tab, "VM");
                push_field(&mut host_tab, "hypervisor", &href.hypervisor_ref);
                push_editable(
                    &mut host_tab,
                    "vmid",
                    &vm.vmid.to_string(),
                    "deployment",
                    "hypervisor.vm.vmid",
                );
                push_editable(
                    &mut host_tab,
                    "name",
                    &vm.name,
                    "deployment",
                    "hypervisor.vm.name",
                );
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
        push_editable(
            &mut network_tab,
            "gateway",
            &d.network.gateway,
            "deployment",
            "network.gateway",
        );
        push_editable(
            &mut network_tab,
            "infra_ip",
            &d.network.infra_ip,
            "deployment",
            "network.infra_ip",
        );
        push_editable(
            &mut network_tab,
            "rack_subnet",
            d.network
                .rack_subnet
                .as_deref()
                .unwrap_or("fd00:1122:3344:0100::/56"),
            "deployment",
            "network.rack_subnet",
        );
        push_editable(
            &mut network_tab,
            "uplink_speed",
            d.network
                .uplink_port_speed
                .as_deref()
                .unwrap_or("40G"),
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
            d.network
                .external_dns_zone_name
                .as_deref()
                .unwrap_or("oxide.test"),
            "deployment",
            "network.external_dns_zone_name",
        );
        push_editable(
            &mut network_tab,
            "dns_resolvers",
            &d.network
                .dns_servers
                .as_ref()
                .map(|v| v.join(", "))
                .unwrap_or_else(|| "1.1.1.1, 9.9.9.9".into()),
            "deployment",
            "network.dns_servers",
        );
        push_editable(
            &mut network_tab,
            "ntp_servers",
            &d.network
                .ntp_servers
                .as_ref()
                .map(|v| v.join(", "))
                .unwrap_or_else(|| "0.pool.ntp.org".into()),
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
            d.network
                .allowed_source_ips
                .as_deref()
                .unwrap_or("any"),
            "deployment",
            "network.allowed_source_ips",
        );

        // ── Build tab ──
        let mut build_tab = Vec::new();

        push_header(&mut build_tab, "OMICRON");
        push_editable(
            &mut build_tab,
            "repo_path",
            &b.omicron.repo_path,
            "build",
            "omicron.repo_path",
        );
        push_editable(
            &mut build_tab,
            "repo_url",
            b.omicron
                .repo_url
                .as_deref()
                .unwrap_or("https://github.com/oxidecomputer/omicron.git"),
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
                repo_url: b
                    .omicron
                    .repo_url
                    .clone()
                    .unwrap_or_else(|| {
                        "https://github.com/oxidecomputer/omicron.git".into()
                    }),
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
            &b.omicron
                .overrides
                .cockroachdb_redundancy
                .map(|v| v.to_string())
                .unwrap_or_default(),
            "build",
            "omicron.overrides.cockroachdb_redundancy",
        );
        push_editable(
            &mut build_tab,
            "vdev_count",
            &b.omicron
                .overrides
                .vdev_count
                .map(|v| v.to_string())
                .unwrap_or_default(),
            "build",
            "omicron.overrides.vdev_count",
        );
        push_editable(
            &mut build_tab,
            "vdev_size_bytes",
            &b.omicron
                .overrides
                .vdev_size_bytes
                .map(|v| v.to_string())
                .unwrap_or_default(),
            "build",
            "omicron.overrides.vdev_size_bytes",
        );
        push_editable(
            &mut build_tab,
            "storage_buffer_gib",
            &b.omicron
                .overrides
                .control_plane_storage_buffer_gib
                .map(|v| v.to_string())
                .unwrap_or_default(),
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
                &propolis
                    .patched
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "(not set)".into()),
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
                &propolis
                    .source
                    .as_ref()
                    .map(|s| format!("{s:?}"))
                    .unwrap_or_else(|| "(not set)".into()),
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
        push_editable(
            &mut nexus_tab,
            "silo_name",
            &nx.silo_name,
            "deployment",
            "nexus.silo_name",
        );
        push_editable(
            &mut nexus_tab,
            "username",
            &nx.username,
            "deployment",
            "nexus.username",
        );
        push_editable(
            &mut nexus_tab,
            "password",
            &nx.password,
            "deployment",
            "nexus.password",
        );

        push_header(&mut nexus_tab, "IP POOL");
        push_editable(
            &mut nexus_tab,
            "pool_name",
            &nx.ip_pool_name,
            "deployment",
            "nexus.ip_pool_name",
        );

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
        let mon = &self.config.monitoring;

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
        if let Some(drift) = &self.state.drift {
            push_header(&mut monitoring_tab, "DRIFT");
            push_field(&mut monitoring_tab, "last_checked", &drift.last_checked);
        }

        // Tuning fields appended to build tab
        let t = &b.tuning;

        push_header(&mut build_tab, "TUNING");
        push_editable(
            &mut build_tab,
            "svcadm_autoclear",
            &t.svcadm_autoclear
                .map(|v| v.to_string())
                .unwrap_or_else(|| "false".into()),
            "build",
            "tuning.svcadm_autoclear",
        );
        push_editable(
            &mut build_tab,
            "swap_size_gb",
            &t.swap_size_gb
                .map(|v| v.to_string())
                .unwrap_or_else(|| "8".into()),
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
            &t.memory_earmark_mb
                .map(|v| v.to_string())
                .unwrap_or_else(|| "6144".into()),
            "build",
            "tuning.memory_earmark_mb",
        );
        push_editable(
            &mut build_tab,
            "vmm_reservoir_%",
            &t.vmm_reservoir_percentage
                .map(|v| v.to_string())
                .unwrap_or_else(|| "60".into()),
            "build",
            "tuning.vmm_reservoir_percentage",
        );
        push_editable(
            &mut build_tab,
            "swap_device_size_gb",
            &t.swap_device_size_gb
                .map(|v| v.to_string())
                .unwrap_or_else(|| "64".into()),
            "build",
            "tuning.swap_device_size_gb",
        );

        self.tab_lines = [host_tab, network_tab, build_tab, nexus_tab, monitoring_tab];
    }
}

impl ConfigPanel for DeploymentPanel {
    fn title(&self) -> String {
        self.name.clone()
    }

    fn is_capturing(&self) -> bool {
        !matches!(self.edit_mode, EditMode::Viewing)
    }

    fn handle_key(&mut self, key: KeyEvent) -> PanelEvent {
        // Overlay: git ref picker / fetching — captures all input
        if matches!(
            self.edit_mode,
            EditMode::GitRefSelect { .. } | EditMode::Fetching
        ) {
            return self.handle_git_ref_key(key);
        }

        // Overlay: inline text editing — captures all input
        if let EditMode::Editing { .. } = self.edit_mode {
            match key.code {
                KeyCode::Enter => {
                    if let Err(msg) = self.confirm_edit() {
                        return PanelEvent::Action(PanelAction::Error(msg));
                    }
                }
                KeyCode::Esc => self.cancel_edit(),
                _ => self.handle_edit_event(key),
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
            KeyCode::Enter => {
                if let Some(action) = self.start_edit() {
                    PanelEvent::Action(action)
                } else {
                    PanelEvent::Consumed
                }
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if self.is_first_tab() {
                    PanelEvent::Ignored // Let ConfigView move focus to left panel
                } else {
                    self.prev_tab();
                    self.focus_first_editable();
                    PanelEvent::Consumed
                }
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if self.is_last_tab() {
                    PanelEvent::Ignored
                } else {
                    self.next_tab();
                    self.focus_first_editable();
                    PanelEvent::Consumed
                }
            }
            KeyCode::Esc => PanelEvent::Ignored, // Let ConfigView handle
            _ => PanelEvent::Ignored,
        }
    }

    fn render(&self, frame: &mut Frame, area: Rect, palette: &Palette) {
        let p = palette;

        // Split: tab bar (1 line) + content
        let chunks =
            Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);

        // Tab bar
        self.render_tab_row(frame, chunks[0], p);

        let content_area = chunks[1];
        let detail_lines = self.detail_lines();

        if detail_lines.is_empty() {
            frame.render_widget(
                Paragraph::new("  No configuration loaded.")
                    .style(Style::default().fg(p.text_tertiary).bg(p.bg_panel)),
                content_area,
            );
            return;
        }

        let edit_input = if let EditMode::Editing { ref input } = self.edit_mode {
            Some(input)
        } else {
            None
        };

        let vis_h = render_detail_lines(
            frame,
            content_area,
            detail_lines,
            self.selected_line(),
            self.scroll_offset(),
            true, // always focused when we're rendering panel content
            edit_input,
            p,
        );
        self.visible_height.set(vis_h);

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

    fn receive_data(&mut self, data: PanelData) {
        match data {
            PanelData::GitRefs(refs) => {
                let current = self
                    .detail_lines()
                    .get(self.selected_line())
                    .and_then(|l| l.raw_value.as_deref())
                    .and_then(|v| if v == "HEAD" { None } else { Some(v) });
                let selector = GitRefSelector::new(current, refs);
                self.edit_mode = EditMode::GitRefSelect { selector };
            }
            _ => {} // Ignore data types not relevant to this panel
        }
    }
}
