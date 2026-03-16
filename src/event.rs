use crate::ops::recover::RecoveryEvent;
use crate::ops::status::HostStatus;

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
}

#[derive(Debug, Clone)]
pub enum BuildEvent {
    StepStarted(String),
    StepDetail(String, String),
    StepCompleted(String),
    StepFailed(String, String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}
