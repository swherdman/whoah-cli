use crate::ops::recover::RecoveryEvent;
use crate::ops::status::HostStatus;

#[derive(Debug)]
pub enum Action {
    Quit,
    Resize(u16, u16),
    // Navigation
    FocusNext,
    FocusPrev,
    ScrollUp,
    ScrollDown,
    // Commands
    StartRecovery,
    RefreshStatus,
    ShowHelp,
    // State updates (from async tasks)
    UpdateStatus(Box<HostStatus>),
    RecoveryProgress(RecoveryEvent),
}
