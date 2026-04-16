//! SSH client handler for russh.
//!
//! Implements the `russh::client::Handler` trait which handles server-side
//! events during an SSH session (host key verification, channel events, etc.).

use russh::keys::ssh_key::PublicKey;

/// Handler for russh client events.
///
/// Currently accepts all host keys (matching the previous openssh
/// `KnownHosts::Accept` behavior). Can be extended for host key
/// verification, ProxyCommand support, etc.
pub struct SshClientHandler;

impl Default for SshClientHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl SshClientHandler {
    pub fn new() -> Self {
        Self
    }
}

impl russh::client::Handler for SshClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        // Accept all host keys — matches previous KnownHosts::Accept behavior.
        Ok(true)
    }
}
