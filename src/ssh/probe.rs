//! Lightweight SSH credential probe — tests connectivity and auth
//! without establishing a persistent session.
//!
//! Uses russh to make a one-shot connection, attempt authentication,
//! and classify the result.

use std::sync::Arc;
use std::time::Duration;

use super::auth::authenticate;
use super::handler::SshClientHandler;

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

/// Probe SSH connectivity to `user@host` with a lightweight one-shot connection.
///
/// Returns within `timeout_secs` (default 5).
pub async fn probe_ssh(host: &str, user: &str, timeout_secs: u32) -> SshProbeStatus {
    if host.is_empty() {
        return SshProbeStatus::Offline;
    }

    let config = Arc::new(russh::client::Config::default());
    let addr = format!("{host}:22");
    let handler = SshClientHandler::new();

    // Connect with timeout
    let handle_result = tokio::time::timeout(
        Duration::from_secs(timeout_secs as u64),
        russh::client::connect(config, &addr, handler),
    )
    .await;

    let mut handle = match handle_result {
        Ok(Ok(h)) => h,
        Ok(Err(_)) | Err(_) => return SshProbeStatus::Offline,
    };

    // Try auth
    match authenticate(&mut handle, user).await {
        Ok(()) => {
            let _ = handle
                .disconnect(russh::Disconnect::ByApplication, "", "en")
                .await;
            SshProbeStatus::Valid
        }
        Err(_) => SshProbeStatus::AuthFailed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_probe_empty_host() {
        assert_eq!(probe_ssh("", "root", 1).await, SshProbeStatus::Offline);
    }
}
