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
    Resize(u16, u16),
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
    ShowHelp,
    // State updates (from async tasks)
    UpdateStatus(Box<HostStatus>),
    RecoveryProgress(RecoveryEvent),
}
