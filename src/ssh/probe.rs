//! Lightweight SSH credential probe — tests connectivity and auth
//! without establishing a persistent mux session.
//!
//! Uses a one-shot `ssh -o BatchMode=yes ... true` command to avoid
//! the mux channel refusal bug (see docs/BUG-ssh-mux-channel-refusal.md).

use tokio::process::Command;

/// Result of an SSH credential probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshProbeStatus {
    /// Not yet checked (gray dot).
    Unknown,
    /// Probe is in-flight (gray dot).
    Checking,
    /// SSH connection succeeded (green dot).
    Valid,
    /// Authentication refused — wrong key or user (red dot).
    AuthFailed,
    /// Host unreachable — connection refused or timed out (yellow dot).
    Offline,
}

/// Probe SSH connectivity to `user@host` with a lightweight one-shot command.
///
/// Does NOT use openssh crate or ControlMaster — avoids mux session issues.
/// Returns within `timeout_secs` (default 5).
pub async fn probe_ssh(host: &str, user: &str, timeout_secs: u32) -> SshProbeStatus {
    if host.is_empty() {
        return SshProbeStatus::Offline;
    }

    let output = Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            "-o", &format!("ConnectTimeout={timeout_secs}"),
            "-o", "StrictHostKeyChecking=accept-new",
            // Disable mux — we want a clean one-shot connection
            "-o", "ControlMaster=no",
            "-o", "ControlPath=none",
            &format!("{user}@{host}"),
            "true",
        ])
        .output()
        .await;

    match output {
        Ok(result) => {
            if result.status.success() {
                return SshProbeStatus::Valid;
            }

            let stderr = String::from_utf8_lossy(&result.stderr);
            classify_ssh_error(&stderr)
        }
        Err(_) => {
            // ssh binary not found or other OS error
            SshProbeStatus::Offline
        }
    }
}

/// Classify an SSH error based on stderr output.
/// Extracted as a pure function for testability.
fn classify_ssh_error(stderr: &str) -> SshProbeStatus {
    let lower = stderr.to_lowercase();
    if lower.contains("permission denied")
        || lower.contains("publickey")
        || lower.contains("authentication")
        || lower.contains("no more authentication methods")
    {
        SshProbeStatus::AuthFailed
    } else {
        SshProbeStatus::Offline
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_permission_denied() {
        assert_eq!(
            classify_ssh_error("user@host: Permission denied (publickey)."),
            SshProbeStatus::AuthFailed,
        );
    }

    #[test]
    fn test_classify_publickey() {
        assert_eq!(
            classify_ssh_error("Permission denied (publickey,keyboard-interactive)."),
            SshProbeStatus::AuthFailed,
        );
    }

    #[test]
    fn test_classify_no_more_methods() {
        assert_eq!(
            classify_ssh_error("Received disconnect: No more authentication methods available"),
            SshProbeStatus::AuthFailed,
        );
    }

    #[test]
    fn test_classify_connection_refused() {
        assert_eq!(
            classify_ssh_error("ssh: connect to host 10.0.0.1 port 22: Connection refused"),
            SshProbeStatus::Offline,
        );
    }

    #[test]
    fn test_classify_connection_timed_out() {
        assert_eq!(
            classify_ssh_error("ssh: connect to host 10.0.0.1 port 22: Connection timed out"),
            SshProbeStatus::Offline,
        );
    }

    #[test]
    fn test_classify_host_not_found() {
        assert_eq!(
            classify_ssh_error("ssh: Could not resolve hostname badhost: Name or service not known"),
            SshProbeStatus::Offline,
        );
    }

    #[test]
    fn test_classify_empty_stderr() {
        assert_eq!(classify_ssh_error(""), SshProbeStatus::Offline);
    }

    #[tokio::test]
    async fn test_probe_empty_host() {
        assert_eq!(probe_ssh("", "root", 1).await, SshProbeStatus::Offline);
    }
}
