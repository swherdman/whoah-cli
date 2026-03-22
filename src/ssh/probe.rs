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

            let stderr = String::from_utf8_lossy(&result.stderr).to_lowercase();

            if stderr.contains("permission denied")
                || stderr.contains("publickey")
                || stderr.contains("authentication")
                || stderr.contains("no more authentication methods")
            {
                SshProbeStatus::AuthFailed
            } else {
                // Connection refused, timeout, host not found, etc.
                SshProbeStatus::Offline
            }
        }
        Err(_) => {
            // ssh binary not found or other OS error
            SshProbeStatus::Offline
        }
    }
}
