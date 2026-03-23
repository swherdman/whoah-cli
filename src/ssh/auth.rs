//! SSH authentication for russh sessions.
//!
//! Supports multiple authentication methods in order:
//! 1. SSH agent (if available)
//! 2. Identity files from well-known paths (~/.ssh/id_ed25519, id_rsa, id_ecdsa)

use std::sync::Arc;

use color_eyre::{eyre::eyre, Result};
use russh::client::{AuthResult, Handle};
use russh::keys::key::PrivateKeyWithHashAlg;

use super::handler::SshClientHandler;

/// Authenticate an SSH session using available methods.
///
/// Tries ssh-agent first, then falls back to well-known identity files.
/// Returns Ok(()) on success, Err if no method succeeds.
pub async fn authenticate(handle: &mut Handle<SshClientHandler>, user: &str) -> Result<()> {
    // 1. Try ssh-agent
    match try_agent_auth(handle, user).await {
        Ok(true) => return Ok(()),
        Ok(false) => tracing::debug!("ssh-agent: no accepted keys"),
        Err(e) => tracing::debug!("ssh-agent unavailable: {e}"),
    }

    // 2. Try well-known identity files
    if try_key_file_auth(handle, user).await? {
        return Ok(());
    }

    Err(eyre!(
        "SSH authentication failed for {user}: no agent keys or identity files accepted"
    ))
}

/// Try authenticating via ssh-agent.
/// Returns Ok(true) if auth succeeded, Ok(false) if agent had no accepted keys,
/// Err if agent is unavailable.
async fn try_agent_auth(handle: &mut Handle<SshClientHandler>, user: &str) -> Result<bool> {
    let mut agent = russh::keys::agent::client::AgentClient::connect_env().await?;
    let identities = agent.request_identities().await?;

    tracing::debug!("ssh-agent has {} identities", identities.len());

    for key in identities {
        let public_key = key.clone();
        // best_supported_rsa_hash returns Result<Option<Option<HashAlg>>>
        // Outer Option: None if server doesn't support ext-info
        // Inner Option: None if no RSA hash preference
        // Flatten to Option<HashAlg> for the auth call
        let hash_alg = handle
            .best_supported_rsa_hash()
            .await
            .unwrap_or(None)
            .flatten();

        let result = handle
            .authenticate_publickey_with(user, public_key, hash_alg, &mut agent)
            .await;

        match result {
            Ok(AuthResult::Success) => {
                tracing::info!("ssh-agent auth succeeded for {user}");
                return Ok(true);
            }
            Ok(AuthResult::Failure { .. }) => continue,
            Err(e) => {
                tracing::debug!("ssh-agent key rejected: {e}");
                continue;
            }
        }
    }

    Ok(false)
}

/// Try authenticating with identity files from well-known paths.
/// Returns Ok(true) if auth succeeded, Ok(false) if no key was accepted.
async fn try_key_file_auth(handle: &mut Handle<SshClientHandler>, user: &str) -> Result<bool> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
    let key_paths = [
        format!("{home}/.ssh/id_ed25519"),
        format!("{home}/.ssh/id_rsa"),
        format!("{home}/.ssh/id_ecdsa"),
    ];

    for path in &key_paths {
        let key = match russh::keys::load_secret_key(path, None) {
            Ok(k) => k,
            Err(_) => continue,
        };

        tracing::debug!("Trying identity file: {path}");

        let hash_alg = handle
            .best_supported_rsa_hash()
            .await
            .unwrap_or(None)
            .flatten();

        let key_with_hash = PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg);

        match handle.authenticate_publickey(user, key_with_hash).await {
            Ok(AuthResult::Success) => {
                tracing::info!("Key file auth succeeded: {path}");
                return Ok(true);
            }
            Ok(AuthResult::Failure { .. }) => {
                tracing::debug!("Key file rejected: {path}");
                continue;
            }
            Err(e) => {
                tracing::debug!("Key file auth error for {path}: {e}");
                continue;
            }
        }
    }

    Ok(false)
}
