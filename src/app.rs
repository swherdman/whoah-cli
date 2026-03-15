use std::sync::Arc;
use std::time::Instant;

use color_eyre::Result;
use ratatui::crossterm::event::{Event as CtEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::prelude::*;
use tokio::sync::mpsc;

use crate::action::Action;
use crate::config::DeploymentConfig;
use crate::event::{AppEvent, Event, Severity};
use crate::ops::recover::{run_recovery, RecoveryEvent, RecoveryParams};
use crate::ops::status::gather_status;
use crate::ssh::session::SshHost;
use crate::ssh::RemoteHost;
use crate::tui::components::alert_bar::AlertBar;
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
enum AppMode {
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
    mode: AppMode,
    focused: FocusedPanel,

    // Components
    status_panel: StatusPanel,
    disk_panel: DiskPanel,
    log_panel: LogPanel,
    alert_bar: AlertBar,
    status_bar: StatusBarComponent,
    recovery_view: RecoveryView,

    // SSH state
    host: Option<Arc<SshHost>>,
    last_poll: Option<Instant>,

    // Event channel for async tasks to push events
    app_event_tx: Option<mpsc::UnboundedSender<Event>>,
}

impl App {
    pub fn new(config: DeploymentConfig, deployment_name: String) -> Self {
        let thresholds = config.monitoring.thresholds.clone();
        Self {
            config,
            deployment_name: deployment_name.clone(),
            should_quit: false,
            mode: AppMode::Dashboard,
            focused: FocusedPanel::Status,
            status_panel: StatusPanel::new(),
            disk_panel: DiskPanel::new(thresholds),
            log_panel: LogPanel::new(),
            alert_bar: AlertBar::new(),
            status_bar: StatusBarComponent::new(&deployment_name),
            recovery_view: RecoveryView::new(),
            host: None,
            last_poll: None,
            app_event_tx: None,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        let mut tui = Tui::new()?;
        tui.enter()?;

        self.app_event_tx = Some(tui.event_tx());
        self.log("Starting whoah dashboard...");

        // Connect to host
        self.connect().await;

        // Initial status poll
        self.spawn_status_poll();

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
                self.log("ERROR: No hosts configured");
                return;
            }
        };

        self.log(&format!(
            "Connecting to {}@{}...",
            host_config.ssh_user, host_config.address
        ));

        match SshHost::connect(&host_config).await {
            Ok(host) => {
                self.status_bar.set_connected(&host_config.address);
                self.log(&format!("Connected to {}", host_config.address));
                self.host = Some(Arc::new(host));
            }
            Err(e) => {
                self.log(&format!("Connection failed: {e}"));
                self.alert_bar.set_alert(
                    Severity::Critical,
                    format!("SSH connection failed: {e}"),
                );
            }
        }
    }

    fn handle_event(&mut self, event: &Event) -> Option<Action> {
        match event {
            Event::Terminal(CtEvent::Key(key)) => self.handle_key(*key),
            Event::Terminal(CtEvent::Resize(w, h)) => Some(Action::Resize(*w, *h)),
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

        // Ctrl+C quits from anywhere
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return Some(Action::Quit);
        }

        match self.mode {
            AppMode::Dashboard => match key.code {
                KeyCode::Char('q') => Some(Action::Quit),
                KeyCode::Esc => Some(Action::Quit),
                KeyCode::Tab => Some(Action::FocusNext),
                KeyCode::BackTab => Some(Action::FocusPrev),
                KeyCode::Char('j') | KeyCode::Down => Some(Action::ScrollDown),
                KeyCode::Char('k') | KeyCode::Up => Some(Action::ScrollUp),
                KeyCode::Char('s') => Some(Action::RefreshStatus),
                KeyCode::Char('r') => Some(Action::StartRecovery),
                _ => None,
            },
            AppMode::Recovery => match key.code {
                KeyCode::Esc => {
                    self.mode = AppMode::Dashboard;
                    self.recovery_view.deactivate();
                    None
                }
                KeyCode::Char('q') => Some(Action::Quit),
                KeyCode::Char('j') | KeyCode::Down => Some(Action::ScrollDown),
                KeyCode::Char('k') | KeyCode::Up => Some(Action::ScrollUp),
                _ => None,
            },
        }
    }

    fn handle_action(&mut self, action: Action) {
        match action {
            Action::Quit => {
                self.should_quit = true;
            }
            Action::FocusNext => {
                self.focused = match self.focused {
                    FocusedPanel::Status => FocusedPanel::Disk,
                    FocusedPanel::Disk => FocusedPanel::Status,
                };
            }
            Action::FocusPrev => {
                self.focused = match self.focused {
                    FocusedPanel::Status => FocusedPanel::Disk,
                    FocusedPanel::Disk => FocusedPanel::Status,
                };
            }
            Action::ScrollUp | Action::ScrollDown => {
                match self.mode {
                    AppMode::Recovery => self.recovery_view.update(&action),
                    AppMode::Dashboard => match self.focused {
                        FocusedPanel::Status => self.status_panel.update(&action),
                        FocusedPanel::Disk => self.disk_panel.update(&action),
                    },
                }
            }
            Action::RefreshStatus => {
                self.log("Manual status refresh...");
                self.spawn_status_poll();
            }
            Action::StartRecovery => {
                self.log("Starting recovery...");
                self.mode = AppMode::Recovery;
                self.recovery_view.start();
                self.spawn_recovery();
            }
            Action::RecoveryProgress(ref event) => {
                self.recovery_view.update(&action);
                // Log key events
                match event {
                    RecoveryEvent::StepCompleted(step, dur) => {
                        self.log(&format!(
                            "Recovery: {} completed ({:.1}s)",
                            step.label(),
                            dur.as_secs_f64()
                        ));
                    }
                    RecoveryEvent::StepFailed { step, error, .. } => {
                        self.log(&format!("Recovery: {} FAILED: {}", step.label(), error));
                    }
                    RecoveryEvent::RecoveryComplete(dur) => {
                        self.log(&format!(
                            "Recovery complete in {:.0}s",
                            dur.as_secs_f64()
                        ));
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

                self.log(&format!(
                    "Status: {} zones, rpool {}%",
                    status.zones.service_counts.values().sum::<u32>(),
                    status.disk.rpool.as_ref().map(|r| r.capacity_pct).unwrap_or(0),
                ));
            }
            _ => {}
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
            self.log("Cannot start recovery: not connected");
            self.mode = AppMode::Dashboard;
            self.recovery_view.deactivate();
            return;
        };
        let Some(tx) = self.app_event_tx.clone() else {
            return;
        };

        let params = match RecoveryParams::from_config(&self.config) {
            Ok(p) => p,
            Err(e) => {
                self.log(&format!("Cannot start recovery: {e}"));
                self.mode = AppMode::Dashboard;
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

    fn render(&mut self, frame: &mut Frame) {
        let p = Palette::default();

        // Fill entire frame with base background
        frame.render_widget(
            ratatui::widgets::Block::default().style(Style::default().bg(p.bg_base)),
            frame.area(),
        );

        // Update focus state on panels
        self.status_panel.set_focused(self.focused == FocusedPanel::Status);
        self.disk_panel.set_focused(self.focused == FocusedPanel::Disk);

        match self.mode {
            AppMode::Dashboard => self.render_dashboard(frame),
            AppMode::Recovery => self.render_recovery(frame),
        }
    }

    fn render_dashboard(&self, frame: &mut Frame) {
        let areas = dashboard_layout(frame.area());

        // Title bar at top — host info, or alert if active
        if self.alert_bar.message.is_some() {
            self.alert_bar.render(frame, areas.title_bar);
        } else {
            self.status_bar.render_title_bar(frame, areas.title_bar);
        }

        // Main panels
        self.status_panel.render(frame, areas.left_panel);
        self.disk_panel.render(frame, areas.right_panel);

        // Log panel
        self.log_panel.render(frame, areas.log_panel);

        // Keybindings bar at bottom
        self.status_bar.render_keybindings(frame, areas.keybindings_bar);
    }

    fn render_recovery(&self, frame: &mut Frame) {
        let area = frame.area();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(5),
                Constraint::Length(1),
            ])
            .split(area);

        self.status_bar.render_title_bar(frame, chunks[0]);
        self.recovery_view.render(frame, chunks[1]);
        self.status_bar.render_keybindings(frame, chunks[2]);
    }

    fn log(&mut self, message: &str) {
        let timestamp = chrono::Local::now().format("%H:%M:%S");
        self.log_panel.push(format!("[{timestamp}] {message}"));
        tracing::info!("{message}");
    }
}
