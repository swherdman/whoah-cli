use crate::ops::recover::RecoveryEvent;
use crate::ops::status::HostStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Build,
    Config,
    Monitor,
    Debug,
}

#[derive(Debug)]
pub enum Action {
    Quit,
    // Navigation
    SwitchScreen(Screen),
    FocusNext,
    FocusPrev,
    ScrollUp,
    ScrollDown,
    // Commands
    StartBuild,
    StartRecovery,
    RefreshStatus,
    // State updates (from async tasks)
    UpdateStatus(Box<HostStatus>),
    RecoveryProgress(RecoveryEvent),
}

/// Key event routing result, following the Helix compositor pattern.
/// Components return this to indicate whether they consumed the event.
pub enum EventResult {
    /// Event was not handled — parent should continue routing.
    Ignored,
    /// Event was handled — stop routing. Optionally produce an Action.
    Consumed(Option<Action>),
}
