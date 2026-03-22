use std::sync::Arc;
use std::time::Instant;

use color_eyre::Result;
use ratatui::crossterm::event::{Event as CtEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::prelude::*;
use tokio::sync::mpsc;

use crate::action::{Action, EventResult, Screen};
use crate::config::DeploymentConfig;
use crate::event::{AppEvent, Event, Severity};
use crate::git::RefCache;
use crate::ops::pipeline::{self, Pipeline};
use crate::parse::cargo_progress::{self, CargoTracker};
use crate::parse::omicron_pkg_log::{self, OmicronPkgTracker};
use crate::parse::pkg_progress;
use crate::parse::xtask_download::{self, XtaskTracker};
use crate::ops::recover::{run_recovery, RecoveryEvent, RecoveryParams};
use crate::ops::status::gather_status;
use crate::ssh::session::SshHost;
use crate::ssh::RemoteHost;
use crate::tui::components::alert_bar::AlertBar;
use crate::tui::components::build_view::BuildView;
use crate::tui::components::config_view::ConfigView;
use crate::tui::components::debug_view::DebugView;
use crate::tui::components::disk_panel::DiskPanel;
use crate::tui::components::log_panel::LogPanel;
use crate::tui::components::recovery_view::RecoveryView;
use crate::tui::components::status_bar::StatusBarComponent;
use crate::tui::components::status_panel::StatusPanel;
use crate::tui::components::Component;
use crate::tui::layout::dashboard_layout;
use crate::tui::theme::Palette;
use crate::tui::Tui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MonitorMode {
    Dashboard,
    Recovery,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusedPanel {
    Status,
    Disk,
}

pub struct App {
    config: DeploymentConfig,
    deployment_name: String,
    should_quit: bool,
    screen: Screen,
    monitor_mode: MonitorMode,
    focused: FocusedPanel,

    // Components
    status_panel: StatusPanel,
    disk_panel: DiskPanel,
    log_panel: LogPanel,
    alert_bar: AlertBar,
    status_bar: StatusBarComponent,
    recovery_view: RecoveryView,
    build_view: BuildView,
    config_view: ConfigView,
    debug_view: DebugView,
    pipeline: Pipeline,

    // SSH state
    host: Option<Arc<SshHost>>,
    last_poll: Option<Instant>,
    needs_reconnect: bool,
    last_status_log: Option<String>,

    // Event channel for async tasks to push events
    app_event_tx: Option<mpsc::UnboundedSender<Event>>,

    // Build step parser state
    cargo_tracker: CargoTracker,
    xtask_tracker: XtaskTracker,
    omicron_pkg_tracker: OmicronPkgTracker,

    // Git ref cache for ref selectors (session-scoped, keyed by repo URL)
    git_ref_cache: RefCache,

    // Demo mode: simulate build pipeline without SSH
    demo: bool,
}

impl App {
    pub fn new(config: DeploymentConfig, deployment_name: String, demo: bool) -> Self {
        let thresholds = config.monitoring.thresholds.clone();
        let vdev_size = config.build.omicron.overrides.vdev_size_bytes.unwrap_or(42949672960);
        Self {
            config,
            deployment_name: deployment_name.clone(),
            should_quit: false,
            screen: Screen::Config,
            monitor_mode: MonitorMode::Dashboard,
            focused: FocusedPanel::Status,
            status_panel: StatusPanel::new(),
            disk_panel: DiskPanel::new(thresholds, vdev_size),
            log_panel: LogPanel::new(),
            alert_bar: AlertBar::new(),
            status_bar: StatusBarComponent::new(&deployment_name),
            recovery_view: RecoveryView::new(),
            build_view: BuildView::new(),
            config_view: ConfigView::new(&deployment_name),
            debug_view: DebugView::new(),
            pipeline: pipeline::build_deploy_pipeline(),
            host: None,
            last_poll: None,
            needs_reconnect: false,
            last_status_log: None,
            app_event_tx: None,
            cargo_tracker: CargoTracker::default(),
            xtask_tracker: XtaskTracker::default(),
            omicron_pkg_tracker: OmicronPkgTracker::default(),
            git_ref_cache: RefCache::new(),
            demo,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut tui = Tui::new()?;
        tui.enter()?;

        self.app_event_tx = Some(tui.event_tx());
        tracing::info!("Starting whoah dashboard...");

        if !self.demo {
            // Connect to host
            self.connect().await;

            // Initial status poll
            self.spawn_status_poll();
        } else {
            // Populate Monitor screen with fake status data
            if let Some(tx) = &self.app_event_tx {
                let status = crate::ops::demo::demo_status(&self.config);
                let _ = tx.send(Event::App(AppEvent::StatusUpdated(Box::new(status))));
            }
            let address = self.config.deployment.hosts.values().next()
                .map(|h| h.address.as_str())
                .unwrap_or("192.168.2.100");
            self.status_bar.set_connected(address);
        }

        while !self.should_quit {
            // Wait for next event
            let event = match tui.next_event().await {
                Some(e) => e,
                None => break,
            };

            // Handle the event
            if let Some(action) = self.handle_event(&event) {
                self.handle_action(action);
            }

            // Render on Render events
            if matches!(event, Event::Render) {
                tui.draw(|frame| self.render(frame))?;
            }

            // Tick: check if we should poll again
            if matches!(event, Event::Tick) {
                self.on_tick();
            }
        }

        // Cleanup
        if let Some(host) = self.host.take() {
            if let Ok(host) = Arc::try_unwrap(host) {
                let _ = host.close().await;
            }
        }

        tui.exit()?;
        Ok(())
    }

    async fn connect(&mut self) {
        let host_config = match self.config.deployment.hosts.values().next() {
            Some(h) => h.clone(),
            None => {
                tracing::error!("No hosts configured");
                return;
            }
        };

        tracing::info!("Connecting to {}@{}...", host_config.ssh_user, host_config.address);

        match SshHost::connect(&host_config).await {
            Ok(host) => {
                self.status_bar.set_connected(&host_config.address);
                tracing::info!("Connected to {}", host_config.address);
                self.host = Some(Arc::new(host));
            }
            Err(e) => {
                tracing::error!("Connection failed: {e}");
                self.alert_bar.set_alert(
                    Severity::Critical,
                    format!("SSH connection failed: {e}"),
                );
            }
        }
    }

    fn spawn_connect(&mut self) {
        let host_config = match self.config.deployment.hosts.values().next() {
            Some(h) => h.clone(),
            None => {
                tracing::error!("No hosts configured for active deployment");
                return;
            }
        };
        let Some(tx) = self.app_event_tx.clone() else {
            return;
        };
        tracing::info!(
            "Connecting to {}@{}...",
            host_config.ssh_user,
            host_config.address
        );

        tokio::spawn(async move {
            match SshHost::connect(&host_config).await {
                Ok(host) => {
                    let _ = tx.send(Event::App(AppEvent::Connected {
                        address: host_config.address.clone(),
                        host: Arc::new(host),
                    }));
                }
                Err(e) => {
                    let _ = tx.send(Event::App(AppEvent::Alert {
                        severity: Severity::Critical,
                        message: format!("SSH connection failed: {e}"),
                    }));
                }
            }
        });
    }

    fn handle_event(&mut self, event: &Event) -> Option<Action> {
        match event {
            Event::Terminal(CtEvent::Key(key)) => self.handle_key(*key),
            Event::App(app_event) => {
                self.handle_app_event(app_event);
                None
            }
            _ => None,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Option<Action> {
        if key.kind != KeyEventKind::Press {
            return None;
        }

        // 1. Hard overrides — always fire regardless of input capture
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Some(Action::Quit);
        }

        // 2. Screen-specific handler gets first shot (Helix compositor pattern).
        //    Components that capture input (editors, pickers, modals) return
        //    Consumed here, preventing globals from firing.
        match self.handle_screen_key(key) {
            EventResult::Consumed(action) => return action,
            EventResult::Ignored => {}
        }

        // 3. Global shortcuts — only reached if screen didn't consume the key
        match key.code {
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('1') => Some(Action::SwitchScreen(Screen::Config)),
            KeyCode::Char('2') => Some(Action::SwitchScreen(Screen::Build)),
            KeyCode::Char('3') => Some(Action::SwitchScreen(Screen::Monitor)),
            KeyCode::Char('d') if self.screen != Screen::Debug => {
                Some(Action::SwitchScreen(Screen::Debug))
            }
            _ => None,
        }
    }

    /// Screen-specific key routing. Returns Consumed when the key was handled
    /// (even if no Action is produced), Ignored when the screen doesn't care.
    fn handle_screen_key(&mut self, key: KeyEvent) -> EventResult {
        match self.screen {
            Screen::Config => self.handle_config_key(key),
            Screen::Build => self.handle_build_key(key),
            Screen::Monitor => self.handle_monitor_key(key),
            Screen::Debug => self.handle_debug_key(key),
        }
    }

    fn handle_monitor_key(&mut self, key: KeyEvent) -> EventResult {
        match self.monitor_mode {
            MonitorMode::Dashboard => match key.code {
                KeyCode::Esc => EventResult::Ignored, // falls through to global quit
                KeyCode::Tab => EventResult::Consumed(Some(Action::FocusNext)),
                KeyCode::BackTab => EventResult::Consumed(Some(Action::FocusPrev)),
                KeyCode::Char('j') | KeyCode::Down => EventResult::Consumed(Some(Action::ScrollDown)),
                KeyCode::Char('k') | KeyCode::Up => EventResult::Consumed(Some(Action::ScrollUp)),
                KeyCode::Char('s') => EventResult::Consumed(Some(Action::RefreshStatus)),
                KeyCode::Char('r') => EventResult::Consumed(Some(Action::StartRecovery)),
                _ => EventResult::Ignored,
            },
            MonitorMode::Recovery => match key.code {
                KeyCode::Esc => {
                    self.monitor_mode = MonitorMode::Dashboard;
                    self.recovery_view.deactivate();
                    EventResult::Consumed(None)
                }
                KeyCode::Char('j') | KeyCode::Down => EventResult::Consumed(Some(Action::ScrollDown)),
                KeyCode::Char('k') | KeyCode::Up => EventResult::Consumed(Some(Action::ScrollUp)),
                _ => EventResult::Ignored,
            },
        }
    }

    fn handle_build_key(&mut self, key: KeyEvent) -> EventResult {
        match key.code {
            KeyCode::Esc => EventResult::Ignored, // falls through to global quit
            KeyCode::Char('b') => EventResult::Consumed(Some(Action::StartBuild)),
            KeyCode::Tab => {
                self.build_view.toggle_focus();
                EventResult::Consumed(None)
            }
            KeyCode::Char('j') | KeyCode::Down => {
                if self.build_view.is_log_focused() {
                    self.build_view.scroll_log_down();
                } else {
                    self.build_view.select_next_step();
                }
                EventResult::Consumed(None)
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.build_view.is_log_focused() {
                    self.build_view.scroll_log_up();
                } else {
                    self.build_view.select_prev_step();
                }
                EventResult::Consumed(None)
            }
            _ => EventResult::Ignored,
        }
    }

    fn handle_config_key(&mut self, key: KeyEvent) -> EventResult {
        use crate::tui::components::config_view::ConfigViewEvent;

        match self.config_view.handle_key(key) {
            ConfigViewEvent::Consumed => EventResult::Consumed(None),
            ConfigViewEvent::Ignored => {
                // ConfigView didn't handle it — check for screen-switch Esc
                if key.code == KeyCode::Esc {
                    EventResult::Consumed(Some(Action::SwitchScreen(Screen::Monitor)))
                } else {
                    EventResult::Ignored
                }
            }
            ConfigViewEvent::RequestActivation { name } => {
                let build_running =
                    self.pipeline.started.is_some() && !self.pipeline.is_complete();
                let recovery_running = self.recovery_view.is_active();

                if name != self.deployment_name && (build_running || recovery_running) {
                    tracing::warn!("Cannot switch deployment: operation in progress");
                } else if let Some((name, config)) = self.config_view.activate_selected() {
                    let name = name.to_string();
                    let config = config.clone();
                    if name != self.deployment_name {
                        self.deployment_name = name.clone();
                        self.config = config;
                        self.status_bar.set_deployment(&name);
                        self.needs_reconnect = true;
                        let thresholds = self.config.monitoring.thresholds.clone();
                        let vdev_size = self.config.build.omicron.overrides
                            .vdev_size_bytes.unwrap_or(42949672960);
                        self.disk_panel = DiskPanel::new(thresholds, vdev_size);
                        self.status_panel = StatusPanel::new();
                        tracing::info!("Switched active deployment to {name}");
                    }
                }
                EventResult::Consumed(None)
            }
            ConfigViewEvent::FetchGitRefs { repo_url } => {
                self.fetch_picker_data_for_url(&repo_url);
                EventResult::Consumed(None)
            }
            ConfigViewEvent::ProbeSsh { host, user } => {
                self.spawn_ssh_probe(&host, &user);
                EventResult::Consumed(None)
            }
            ConfigViewEvent::ValidateProxmox { host, user } => {
                self.spawn_proxmox_validation(&host, &user);
                EventResult::Consumed(None)
            }
            ConfigViewEvent::DownloadIso { host, user, iso_storage_path, filename } => {
                self.spawn_iso_download(&host, &user, &iso_storage_path, &filename);
                EventResult::Consumed(None)
            }
            ConfigViewEvent::HypervisorDeleted { name } => {
                tracing::info!("Hypervisor '{name}' deleted");
                EventResult::Consumed(None)
            }
        }
    }

    fn handle_debug_key(&mut self, key: KeyEvent) -> EventResult {
        match key.code {
            KeyCode::Esc => EventResult::Consumed(Some(Action::SwitchScreen(Screen::Monitor))),
            KeyCode::Char('r') => {
                self.debug_view.refresh();
                EventResult::Consumed(None)
            }
            KeyCode::Char('j') | KeyCode::Down => EventResult::Consumed(Some(Action::ScrollDown)),
            KeyCode::Char('k') | KeyCode::Up => EventResult::Consumed(Some(Action::ScrollUp)),
            _ => EventResult::Ignored,
        }
    }

    fn handle_action(&mut self, action: Action) {
        match action {
            Action::Quit => {
                self.should_quit = true;
            }
            Action::SwitchScreen(screen) => {
                self.screen = screen;
                if self.needs_reconnect
                    && matches!(screen, Screen::Monitor | Screen::Build)
                {
                    self.needs_reconnect = false;
                    // Disconnect old host
                    if let Some(host) = self.host.take() {
                        if let Ok(h) = Arc::try_unwrap(host) {
                            tokio::spawn(async move {
                                let _ = h.close().await;
                            });
                        }
                    }
                    self.status_bar.connected = false;
                    self.spawn_connect();
                }
            }
            Action::FocusNext | Action::FocusPrev => {
                self.focused = match self.focused {
                    FocusedPanel::Status => FocusedPanel::Disk,
                    FocusedPanel::Disk => FocusedPanel::Status,
                };
                self.status_panel.set_focused(self.focused == FocusedPanel::Status);
                self.disk_panel.set_focused(self.focused == FocusedPanel::Disk);
            }
            Action::ScrollUp | Action::ScrollDown => {
                match self.screen {
                    Screen::Monitor => match self.monitor_mode {
                        MonitorMode::Recovery => self.recovery_view.update(&action),
                        MonitorMode::Dashboard => match self.focused {
                            FocusedPanel::Status => self.status_panel.update(&action),
                            FocusedPanel::Disk => self.disk_panel.update(&action),
                        },
                    },
                    Screen::Build => self.build_view.update(&action),
                    Screen::Config => {}
                    Screen::Debug => self.debug_view.update(&action),
                }
            }
            Action::RefreshStatus => {
                tracing::info!("Manual status refresh...");
                self.spawn_status_poll();
            }
            Action::StartBuild => {
                let in_progress = self.pipeline.started.is_some()
                    && !self.pipeline.is_complete()
                    && !self.pipeline.has_failure();
                if in_progress {
                    tracing::info!("Build already in progress");
                } else {
                    // Reset pipeline for a fresh run
                    self.pipeline = pipeline::build_deploy_pipeline();
                    tracing::info!("Starting build pipeline...");
                    self.screen = Screen::Build;
                    self.spawn_build();
                }
            }
            Action::StartRecovery => {
                tracing::info!("Starting recovery...");
                self.screen = Screen::Monitor;
                self.monitor_mode = MonitorMode::Recovery;
                self.recovery_view.start();
                self.spawn_recovery();
            }
            Action::RecoveryProgress(ref event) => {
                self.recovery_view.update(&action);
                // Log key events
                match event {
                    RecoveryEvent::StepCompleted(step, dur) => {
                        tracing::info!(
                            "Recovery: {} completed ({:.1}s)",
                            step.label(),
                            dur.as_secs_f64()
                        );
                    }
                    RecoveryEvent::StepFailed { step, error, .. } => {
                        tracing::error!("Recovery: {} FAILED: {}", step.label(), error);
                    }
                    RecoveryEvent::RecoveryComplete(dur) => {
                        tracing::info!(
                            "Recovery complete in {:.0}s",
                            dur.as_secs_f64()
                        );
                        // Trigger a status refresh after recovery
                        self.spawn_status_poll();
                    }
                    _ => {}
                }
            }
            Action::UpdateStatus(status) => {
                self.status_panel.update(&Action::UpdateStatus(status.clone()));
                self.disk_panel.update(&Action::UpdateStatus(status.clone()));

                // Check for alerts
                if status.reboot_detected {
                    self.alert_bar.set_alert(
                        Severity::Critical,
                        "Reboot detected. Press [r] to start recovery.".to_string(),
                    );
                } else {
                    // Check thresholds
                    if let Some(rpool) = &status.disk.rpool {
                        if rpool.capacity_pct >= self.config.monitoring.thresholds.rpool_critical_percent {
                            self.alert_bar.set_alert(
                                Severity::Critical,
                                format!("rpool at {}% — critical!", rpool.capacity_pct),
                            );
                        } else if rpool.capacity_pct >= self.config.monitoring.thresholds.rpool_warning_percent {
                            self.alert_bar.set_alert(
                                Severity::Warning,
                                format!("rpool at {}% — approaching limit", rpool.capacity_pct),
                            );
                        } else {
                            self.alert_bar.clear();
                        }
                    }
                }

                let status_summary = format!(
                    "Status: {} zones, rpool {}%",
                    status.zones.service_counts.values().sum::<u32>(),
                    status.disk.rpool.as_ref().map(|r| r.capacity_pct).unwrap_or(0),
                );
                if self.last_status_log.as_deref() != Some(&status_summary) {
                    tracing::info!("{status_summary}");
                    self.last_status_log = Some(status_summary);
                }
            }
        }
    }

    fn handle_app_event(&mut self, event: &AppEvent) {
        match event {
            AppEvent::StatusUpdated(status) => {
                self.handle_action(Action::UpdateStatus(status.clone()));
            }
            AppEvent::Alert { severity, message } => {
                self.alert_bar.set_alert(severity.clone(), message.clone());
            }
            AppEvent::Recovery(recovery_event) => {
                self.handle_action(Action::RecoveryProgress(recovery_event.clone()));
            }
            AppEvent::Build(build_event) => {
                self.handle_build_event(build_event);
            }
            AppEvent::Connected { address, host } => {
                self.status_bar.set_connected(address);
                self.host = Some(host.clone());
                tracing::info!("Connected to {address}");
                self.spawn_status_poll();
            }
            AppEvent::GitRefsFetched { repo_url, result } => {
                match result {
                    Ok(refs) => {
                        self.git_ref_cache.insert(repo_url.clone(), refs.clone());
                        self.config_view.deliver_data(
                            crate::tui::components::config_detail::PanelData::GitRefs(refs.clone()),
                        );
                    }
                    Err(msg) => {
                        tracing::error!("Failed to fetch refs: {msg}");
                        self.config_view.cancel_fetch();
                    }
                }
            }
            AppEvent::SshProbeResult { host, user, status } => {
                tracing::debug!("SSH probe {user}@{host}: {status:?}");
                let follow_up = self.config_view.deliver_data(
                    crate::tui::components::config_detail::PanelData::SshProbeResult(*status),
                );
                // Chain: SSH valid → trigger Proxmox validation
                if let Some(crate::tui::components::config_view::ConfigViewEvent::ValidateProxmox { host, user }) = follow_up {
                    self.spawn_proxmox_validation(&host, &user);
                }
            }
            AppEvent::DownloadProgress { filename, percent } => {
                self.config_view.deliver_data(
                    crate::tui::components::config_detail::PanelData::DownloadProgress { percent: *percent },
                );
            }
            AppEvent::IsoDownloadResult { filename, result } => {
                match result {
                    Ok(()) => {
                        tracing::info!("ISO download complete: {filename}");
                        self.config_view.deliver_data(
                            crate::tui::components::config_detail::PanelData::DownloadComplete,
                        );
                        if let Some((host, user)) = self.config_view.active_hypervisor_credentials() {
                            self.spawn_proxmox_validation(&host, &user);
                        }
                    }
                    Err(ref msg) => {
                        tracing::error!("ISO download failed: {msg}");
                        self.config_view.deliver_data(
                            crate::tui::components::config_detail::PanelData::DownloadFailed(msg.clone()),
                        );
                    }
                }
            }
            AppEvent::ProxmoxValidated(validation) => {
                tracing::debug!("Proxmox validation: node={:?} disk={:?} iso_storage={:?} iso_file={:?}",
                    validation.node, validation.disk_storage, validation.iso_storage, validation.iso_file);
                self.config_view.deliver_data(
                    crate::tui::components::config_detail::PanelData::ProxmoxValidation(validation.clone()),
                );
            }
        }
    }

    fn handle_build_event(&mut self, event: &crate::event::BuildEvent) {
        use crate::event::BuildEvent;
        match event {
            BuildEvent::StepStarted(id) => {
                let name = self.pipeline.find_step(id)
                    .and_then(|(pi, si)| Some(self.pipeline.phases[pi].steps[si].name))
                    .unwrap_or("unknown");
                self.pipeline.start_step(id);
                tracing::info!("Build: starting {name}");

                // Reset parser trackers for steps that use them
                match id.as_str() {
                    "build-compile" => {
                        self.cargo_tracker = CargoTracker::default();
                    }
                    "build-package" => {
                        self.cargo_tracker = CargoTracker::default();
                        self.omicron_pkg_tracker = OmicronPkgTracker::default();
                    }
                    "build-prereqs-builder" | "build-prereqs-runner" => {
                        self.xtask_tracker = XtaskTracker::default();
                    }
                    _ => {}
                }
            }
            BuildEvent::StepDetail(id, detail) => {
                // Always push raw line to the output buffer
                if let Some(step) = self.pipeline.step_mut(id) {
                    step.push_output(detail.clone());
                }

                // For parser-aware steps, parse the line and show a
                // structured summary instead of the raw output
                let summary = match id.as_str() {
                    "build-compile" => {
                        if let Some(event) = cargo_progress::parse_cargo_line(detail) {
                            self.cargo_tracker.update(&event);
                            Some(self.cargo_tracker.summary())
                        } else {
                            None
                        }
                    }
                    "build-package" => {
                        // build-package gets two streams: cargo Compiling lines
                        // from the main SSH command, and omicron-package LOG
                        // events (Verifying/Downloading) from the background tail.
                        if let Some(event) = cargo_progress::parse_cargo_line(detail) {
                            self.cargo_tracker.update(&event);
                            Some(self.cargo_tracker.summary())
                        } else if let Some(event) = omicron_pkg_log::parse_omicron_pkg_line(detail) {
                            self.omicron_pkg_tracker.update(&event);
                            Some(self.omicron_pkg_tracker.summary())
                        } else {
                            None
                        }
                    }
                    "os-update" | "os-packages" => {
                        if let Some(event) = pkg_progress::parse_pkg_line(detail) {
                            Some(pkg_progress::format_pkg_event(&event))
                        } else {
                            None
                        }
                    }
                    "build-prereqs-builder" | "build-prereqs-runner" => {
                        // Parse both pkg and xtask output
                        if let Some(event) = xtask_download::parse_xtask_line(detail) {
                            self.xtask_tracker.update(&event);
                            Some(self.xtask_tracker.summary())
                        } else if let Some(event) = pkg_progress::parse_pkg_line(detail) {
                            Some(pkg_progress::format_pkg_event(&event))
                        } else {
                            None
                        }
                    }
                    _ => None,
                };

                // Update step detail: use parsed summary if available,
                // otherwise use the raw detail
                let display = summary.unwrap_or_else(|| detail.clone());
                if let Some(step) = self.pipeline.step_mut(id) {
                    if let crate::ops::pipeline::StepStatus::Running { started, .. } = step.status {
                        step.status = crate::ops::pipeline::StepStatus::Running {
                            started,
                            detail: Some(display),
                        };
                    }
                }
            }
            BuildEvent::StepCompleted(id) => {
                let name = self.pipeline.find_step(id)
                    .and_then(|(pi, si)| Some(self.pipeline.phases[pi].steps[si].name))
                    .unwrap_or("unknown");
                self.pipeline.complete_step(id);

                // In demo mode, override the duration with a realistic value
                // and backdate pipeline.started so total_elapsed() matches
                if self.demo {
                    if let Some(step) = self.pipeline.step_mut(id) {
                        step.status = crate::ops::pipeline::StepStatus::Completed {
                            duration: crate::ops::demo::realistic_duration(id),
                        };
                    }
                    let cumulative: std::time::Duration = self.pipeline.phases.iter()
                        .flat_map(|p| &p.steps)
                        .filter_map(|s| s.elapsed())
                        .sum();
                    self.pipeline.started = Some(std::time::Instant::now() - cumulative);
                }

                let dur = self.pipeline.step_mut(id)
                    .and_then(|s| s.elapsed())
                    .unwrap_or_default();
                tracing::info!("Build: {name} completed ({:.1}s)", dur.as_secs_f64());
            }
            BuildEvent::StepFailed(id, error) => {
                self.pipeline.fail_step(id, error.clone());
                tracing::error!("Build: FAILED — {error}");
            }
            BuildEvent::HostDiscovered { address, ssh_user } => {
                tracing::info!("Build discovered host at {address} (user: {ssh_user})");

                // Update the first host entry in the in-memory config
                if let Some(host) = self.config.deployment.hosts.values_mut().next() {
                    host.address = address.clone();
                    host.ssh_user = ssh_user.clone();
                }

                // Persist the new IP to disk
                if let Some(host_name) = self.config.deployment.hosts.keys().next().cloned() {
                    let dep_name = self.deployment_name.clone();
                    let path = format!("hosts.{host_name}.address");
                    if let Err(e) = crate::config::editor::update_deployment_field(
                        &dep_name, "deployment", &path, address,
                    ) {
                        tracing::error!("Failed to persist discovered IP: {e}");
                    }
                }

                // Mark for reconnect so Monitor uses the new IP
                self.needs_reconnect = true;
            }
            BuildEvent::PipelineFinished { success } => {
                if *success && !self.demo {
                    tracing::info!("Build pipeline finished — connecting to new host");
                    // Drop stale connection and connect to the (possibly new) IP
                    if let Some(host) = self.host.take() {
                        if let Ok(h) = Arc::try_unwrap(host) {
                            tokio::spawn(async move {
                                let _ = h.close().await;
                            });
                        }
                    }
                    self.status_bar.connected = false;
                    self.needs_reconnect = false;
                    self.spawn_connect();
                }
            }
        }
    }

    fn on_tick(&mut self) {
        let interval = std::time::Duration::from_secs(
            self.config.monitoring.polling.status_interval_secs,
        );

        let should_poll = match self.last_poll {
            Some(last) => last.elapsed() >= interval,
            None => false, // Initial poll is triggered separately
        };

        if should_poll {
            self.spawn_status_poll();
        }

        // Refresh debug data on tick (not in render path — avoids blocking I/O)
        if self.screen == Screen::Debug && self.debug_view.needs_refresh() {
            self.debug_view.refresh();
        }
    }

    fn spawn_status_poll(&mut self) {
        let Some(host) = self.host.clone() else {
            return;
        };
        let Some(tx) = self.app_event_tx.clone() else {
            return;
        };

        self.last_poll = Some(Instant::now());
        let config = self.config.clone();

        tokio::spawn(async move {
            let host_ref: &dyn RemoteHost = host.as_ref();
            match gather_status(host_ref, &config).await {
                Ok(status) => {
                    let _ = tx.send(Event::App(AppEvent::StatusUpdated(Box::new(status))));
                }
                Err(e) => {
                    tracing::error!("Status poll failed: {e}");
                    let _ = tx.send(Event::App(AppEvent::Alert {
                        severity: Severity::Warning,
                        message: format!("Status poll failed: {e}"),
                    }));
                }
            }
        });
    }

    fn spawn_recovery(&mut self) {
        let Some(host) = self.host.clone() else {
            tracing::warn!("Cannot start recovery: not connected");
            self.monitor_mode = MonitorMode::Dashboard;
            self.recovery_view.deactivate();
            return;
        };
        let Some(tx) = self.app_event_tx.clone() else {
            return;
        };

        let params = match RecoveryParams::from_config(&self.config) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("Cannot start recovery: {e}");
                self.monitor_mode = MonitorMode::Dashboard;
                self.recovery_view.deactivate();
                return;
            }
        };

        let cancel = tokio_util::sync::CancellationToken::new();

        tokio::spawn(async move {
            let (event_tx, mut event_rx) = mpsc::channel::<RecoveryEvent>(256);
            let host_for_recovery = host.clone();

            // Spawn the recovery task
            let recovery_handle = tokio::spawn(async move {
                let host_ref: &dyn RemoteHost = host_for_recovery.as_ref();
                run_recovery(host_ref, &params, event_tx, cancel).await
            });

            // Forward recovery events to the app event loop
            while let Some(event) = event_rx.recv().await {
                let _ = tx.send(Event::App(AppEvent::Recovery(event)));
            }

            // Wait for recovery to finish
            match recovery_handle.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::error!("Recovery failed: {e}");
                }
                Err(e) => {
                    tracing::error!("Recovery task panicked: {e}");
                }
            }
        });
    }

    fn fetch_picker_data_for_url(&mut self, repo_url: &str) {
        // Check session cache first
        if let Some(cached) = self.git_ref_cache.get(repo_url) {
            self.config_view.deliver_data(
                crate::tui::components::config_detail::PanelData::GitRefs(cached.clone()),
            );
            return;
        }

        // Spawn async fetch
        let repo_url = repo_url.to_string();
        if let Some(tx) = self.app_event_tx.clone() {
            let url = repo_url.clone();
            std::thread::spawn(move || {
                let result = crate::git::fetch_repo_refs(&url);
                let _ = tx.send(Event::App(AppEvent::GitRefsFetched {
                    repo_url: url,
                    result,
                }));
            });
        }
    }

    fn spawn_ssh_probe(&mut self, host: &str, user: &str) {
        let Some(tx) = self.app_event_tx.clone() else {
            return;
        };
        let host = host.to_string();
        let user = user.to_string();
        tokio::spawn(async move {
            let status = crate::ssh::probe::probe_ssh(&host, &user, 5).await;
            let _ = tx.send(Event::App(AppEvent::SshProbeResult {
                host,
                user,
                status,
            }));
        });
    }

    fn spawn_proxmox_validation(&mut self, host: &str, user: &str) {
        let Some(tx) = self.app_event_tx.clone() else {
            return;
        };
        let host = host.to_string();
        let user = user.to_string();
        // We need the proxmox config to validate against. Read it from the active panel.
        let proxmox_config = if let Some(panel) = self.config_view.active_hypervisor_proxmox_config() {
            panel
        } else {
            return;
        };
        tokio::spawn(async move {
            let validation = crate::ops::hypervisor_proxmox_validate::validate_proxmox(
                &host, &user, &proxmox_config,
            ).await;
            let _ = tx.send(Event::App(AppEvent::ProxmoxValidated(validation)));
        });
    }

    fn spawn_iso_download(&mut self, host: &str, user: &str, iso_storage_path: &str, filename: &str) {
        let Some(event_tx) = self.app_event_tx.clone() else {
            return;
        };
        let host = host.to_string();
        let user = user.to_string();
        let path = iso_storage_path.to_string();
        let filename = filename.to_string();
        tracing::info!("Starting ISO download: {filename} to {host}:{path}");
        tokio::spawn(async move {
            // Create a channel for progress updates
            let (progress_tx, mut progress_rx) = tokio::sync::mpsc::channel::<crate::ssh::download::DownloadProgress>(16);

            // Forward progress events to the main event loop
            let event_tx_progress = event_tx.clone();
            let filename_progress = filename.clone();
            let progress_forwarder = tokio::spawn(async move {
                while let Some(progress) = progress_rx.recv().await {
                    let _ = event_tx_progress.send(Event::App(AppEvent::DownloadProgress {
                        filename: filename_progress.clone(),
                        percent: progress.percent,
                    }));
                }
            });

            let result = crate::ops::hypervisor_proxmox_validate::download_iso(
                &host, &user, &path, &filename, progress_tx,
            )
            .await
            .map_err(|e| e.to_string());

            // Wait for progress forwarder to finish
            let _ = progress_forwarder.await;

            let _ = event_tx.send(Event::App(AppEvent::IsoDownloadResult {
                filename,
                result,
            }));
        });
    }

    fn spawn_build(&mut self) {
        let Some(tx) = self.app_event_tx.clone() else {
            return;
        };

        if self.demo {
            self.spawn_demo_build(tx);
            return;
        }

        if self.config.deployment.hypervisor.is_none() {
            tracing::warn!("Cannot build: no [hypervisor] section in config");
            return;
        }

        let config = self.config.clone();
        let deploy_name = self.deployment_name.clone();

        // Create an unbounded channel for build events
        let (build_tx, mut build_rx) = mpsc::unbounded_channel::<crate::event::BuildEvent>();

        tokio::spawn(async move {
            // Spawn the deploy task
            let deploy_handle = tokio::spawn(async move {
                crate::ops::deploy::run_deploy(config, deploy_name, build_tx).await
            });

            // Forward build events to the app event loop
            while let Some(event) = build_rx.recv().await {
                let _ = tx.send(Event::App(AppEvent::Build(event)));
            }

            // Wait for deploy to finish
            match deploy_handle.await {
                Ok(Ok(())) => {
                    tracing::info!("Deploy pipeline completed successfully");
                    let _ = tx.send(Event::App(AppEvent::Build(
                        crate::event::BuildEvent::PipelineFinished { success: true },
                    )));
                }
                Ok(Err(e)) => {
                    tracing::error!("Deploy pipeline failed: {e}");
                    let _ = tx.send(Event::App(AppEvent::Build(
                        crate::event::BuildEvent::PipelineFinished { success: false },
                    )));
                    let _ = tx.send(Event::App(AppEvent::Alert {
                        severity: Severity::Warning,
                        message: format!("Build failed: {e}"),
                    }));
                }
                Err(e) => {
                    tracing::error!("Deploy task panicked: {e}");
                    let _ = tx.send(Event::App(AppEvent::Build(
                        crate::event::BuildEvent::PipelineFinished { success: false },
                    )));
                    let _ = tx.send(Event::App(AppEvent::Alert {
                        severity: Severity::Critical,
                        message: format!("Build task panicked: {e}"),
                    }));
                }
            }
        });
    }

    fn spawn_demo_build(&self, tx: mpsc::UnboundedSender<Event>) {
        let (build_tx, mut build_rx) = mpsc::unbounded_channel::<crate::event::BuildEvent>();

        tokio::spawn(async move {
            let demo_handle = tokio::spawn(async move {
                crate::ops::demo::run_demo(build_tx).await
            });

            while let Some(event) = build_rx.recv().await {
                let _ = tx.send(Event::App(AppEvent::Build(event)));
            }

            match demo_handle.await {
                Ok(()) => {
                    let _ = tx.send(Event::App(AppEvent::Build(
                        crate::event::BuildEvent::PipelineFinished { success: true },
                    )));
                }
                Err(e) => {
                    tracing::error!("Demo task panicked: {e}");
                    let _ = tx.send(Event::App(AppEvent::Build(
                        crate::event::BuildEvent::PipelineFinished { success: false },
                    )));
                }
            }
        });
    }

    fn render(&mut self, frame: &mut Frame) {
        let p = Palette::default();

        // Fill entire frame with base background
        frame.render_widget(
            ratatui::widgets::Block::default().style(Style::default().bg(p.bg_base)),
            frame.area(),
        );

        // Common chrome: tab bar (top) + keybindings (bottom)
        let chrome = Layout::vertical([
            Constraint::Length(1), // tab bar
            Constraint::Min(5),    // screen content
            Constraint::Length(1), // keybindings
        ])
        .split(frame.area());

        self.status_bar.render_tab_bar(frame, chrome[0], self.screen);
        let config_editing = self.screen == Screen::Config && self.config_view.is_capturing();
        self.status_bar
            .render_keybindings(frame, chrome[2], self.screen, config_editing);

        // Dispatch to active screen
        match self.screen {
            Screen::Build => self.render_build(frame, chrome[1]),
            Screen::Config => self.render_config(frame, chrome[1]),
            Screen::Monitor => self.render_monitor(frame, chrome[1]),
            Screen::Debug => {
                self.debug_view.render(frame, chrome[1]);
            }
        }
    }

    fn render_build(&mut self, frame: &mut Frame, area: Rect) {
        self.build_view
            .render_pipeline(frame, area, &self.pipeline);
    }

    fn render_config(&self, frame: &mut Frame, area: Rect) {
        self.config_view.render(frame, area);
    }

    fn render_monitor(&mut self, frame: &mut Frame, area: Rect) {
        match self.monitor_mode {
            MonitorMode::Dashboard => self.render_dashboard(frame, area),
            MonitorMode::Recovery => self.render_recovery(frame, area),
        }
    }

    fn render_dashboard(&mut self, frame: &mut Frame, area: Rect) {
        let areas = dashboard_layout(area);

        // Alert or status info
        if self.alert_bar.message.is_some() {
            self.alert_bar.render(frame, areas.title_bar);
        } else {
            // Empty line — reserves space for alerts
            let p = Palette::default();
            frame.render_widget(
                ratatui::widgets::Paragraph::new("")
                    .style(Style::default().bg(p.bg_hover)),
                areas.title_bar,
            );
        }

        // Main panels
        self.status_panel.render(frame, areas.left_panel);
        self.disk_panel.render(frame, areas.right_panel);

        // Log panel
        self.log_panel.render(frame, areas.log_panel);
    }

    fn render_recovery(&self, frame: &mut Frame, area: Rect) {
        self.recovery_view.render(frame, area);
    }
}
