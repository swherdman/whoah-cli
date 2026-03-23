use std::sync::Arc;

use crate::git::RepoRefs;
use crate::ops::hypervisor_proxmox_validate::ProxmoxValidation;
use crate::ops::recover::RecoveryEvent;
use crate::ops::status::HostStatus;
use crate::ssh::probe::SshProbeStatus;
use crate::ssh::session::SshHost;

#[derive(Debug)]
pub enum Event {
    /// Terminal input event
    Terminal(ratatui::crossterm::event::Event),
    /// Periodic tick for business logic
    Tick,
    /// Render frame
    Render,
    /// Application-level event
    App(AppEvent),
}

#[derive(Debug)]
pub enum AppEvent {
    /// Status poll completed
    StatusUpdated(Box<HostStatus>),
    /// Recovery progress
    Recovery(RecoveryEvent),
    /// Build pipeline progress
    Build(BuildEvent),
    /// Alert
    Alert { severity: Severity, message: String },
    /// Async SSH connection completed
    Connected {
        address: String,
        host: Arc<SshHost>,
    },
    /// GitHub API fetch completed for git ref selector
    GitRefsFetched {
        repo_url: String,
        result: Result<RepoRefs, String>,
    },
    /// SSH credential probe completed
    SshProbeResult {
        host: String,
        user: String,
        status: SshProbeStatus,
    },
    /// Proxmox config validation completed
    ProxmoxValidated(ProxmoxValidation),
    /// Prerequisite checks completed
    PrereqsChecked(crate::ops::prereqs::PrereqResults),
    /// ISO download progress update
    DownloadProgress {
        filename: String,
        percent: f32,
    },
    /// Proxmox VM list query completed
    ProxmoxVmList {
        name: String,
        host: String,
        user: String,
        node: String,
        hypervisor_ref: String,
        import: bool,
        result: Result<Vec<crate::ops::hypervisor_proxmox_validate::ProxmoxVm>, String>,
    },
    /// Proxmox VM config query completed (for existing deployment import)
    ProxmoxVmConfig {
        name: String,
        host: String,
        user: String,
        hypervisor_ref: String,
        result: Result<crate::config::types::VmConfig, String>,
    },
    /// Host discovery completed for deployment import
    HostDiscoveryResult {
        name: String,
        host: String,
        user: String,
        hypervisor_ref: Option<String>,
        result: Result<crate::ops::discover::DiscoveredConfig, String>,
    },
    /// ISO download completed
    IsoDownloadResult {
        filename: String,
        result: Result<(), String>,
    },
}

#[derive(Debug, Clone)]
pub enum BuildEvent {
    StepStarted(String),
    StepDetail(String, String),
    StepCompleted(String),
    StepFailed(String, String),
    /// Build discovered the VM's actual IP (may differ from config)
    HostDiscovered { address: String, ssh_user: String },
    /// Build pipeline finished (success or failure)
    PipelineFinished { success: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}
