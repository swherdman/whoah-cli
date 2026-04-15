//! Prerequisite checks for external CLI tools.
//!
//! Runs lightweight checks at startup to verify optional tools
//! are available and configured.

use tokio::process::Command;

/// Status of a prerequisite tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrereqStatus {
    /// Not yet checked.
    Unknown,
    /// Tool is installed and fully functional.
    Ok,
    /// Tool is installed but not fully functional (e.g. daemon not running).
    Degraded,
    /// Tool is not installed or not found.
    Missing,
}

/// Results of all prerequisite checks.
#[derive(Debug, Clone)]
pub struct PrereqResults {
    pub docker: PrereqStatus,
    pub gh: PrereqStatus,
}

impl Default for PrereqResults {
    fn default() -> Self {
        Self {
            docker: PrereqStatus::Unknown,
            gh: PrereqStatus::Unknown,
        }
    }
}

/// Run all prerequisite checks.
pub async fn check_all() -> PrereqResults {
    let (docker, gh) = tokio::join!(check_docker(), check_gh());
    PrereqResults { docker, gh }
}

/// Check if Docker is installed and the daemon is running.
async fn check_docker() -> PrereqStatus {
    // Check if docker binary exists
    let version = Command::new("docker").arg("--version").output().await;

    match version {
        Ok(out) if out.status.success() => {}
        _ => return PrereqStatus::Missing,
    }

    // Check if daemon is running
    let info = Command::new("docker")
        .arg("info")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await;

    match info {
        Ok(out) if out.status.success() => PrereqStatus::Ok,
        _ => PrereqStatus::Degraded, // Installed but daemon not running
    }
}

/// Check if GitHub CLI is installed and authenticated.
async fn check_gh() -> PrereqStatus {
    let output = Command::new("gh")
        .args(["auth", "status"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => PrereqStatus::Ok,
        Ok(_) => PrereqStatus::Degraded, // Installed but not authenticated
        Err(_) => PrereqStatus::Missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prereq_defaults() {
        let r = PrereqResults::default();
        assert_eq!(r.docker, PrereqStatus::Unknown);
        assert_eq!(r.gh, PrereqStatus::Unknown);
    }
}
