//! One-shot SSH command execution.
//!
//! Creates an ephemeral russh connection, authenticates, executes a single
//! command, collects output, and disconnects. Used for lightweight operations
//! that don't need a persistent session (e.g., Proxmox validation, discovery).

use std::sync::Arc;
use std::time::Duration;

use color_eyre::{eyre::eyre, Result};
use russh::ChannelMsg;

use super::auth::authenticate;
use super::handler::SshClientHandler;
use super::CommandOutput;

/// Execute a single command on a remote host via an ephemeral SSH connection.
///
/// Opens a connection, authenticates, runs the command, collects output,
/// and disconnects. Suitable for one-off queries where maintaining a
/// persistent session isn't needed.
pub async fn one_shot(
    host: &str,
    user: &str,
    cmd: &str,
    timeout_secs: u64,
) -> Result<CommandOutput> {
    let config = Arc::new(russh::client::Config::default());
    let addr = format!("{host}:22");
    let handler = SshClientHandler::new();

    let mut handle = tokio::time::timeout(
        Duration::from_secs(10),
        russh::client::connect(config, &addr, handler),
    )
    .await
    .map_err(|_| eyre!("SSH to {host} timed out"))?
    .map_err(|e| eyre!("SSH to {host} failed: {e}"))?;

    authenticate(&mut handle, user)
        .await
        .map_err(|e| eyre!("SSH auth to {user}@{host} failed: {e}"))?;

    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|e| eyre!("Failed to open channel on {host}: {e}"))?;

    channel
        .exec(true, cmd.as_bytes())
        .await
        .map_err(|e| eyre!("Failed to exec on {host}: {e}"))?;

    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let mut exit_code: Option<u32> = None;

    let result = tokio::time::timeout(Duration::from_secs(timeout_secs), async {
        loop {
            match channel.wait().await {
                Some(ChannelMsg::Data { data }) => stdout.extend_from_slice(&data),
                Some(ChannelMsg::ExtendedData { data, ext }) if ext == 1 => {
                    stderr.extend_from_slice(&data)
                }
                Some(ChannelMsg::ExitStatus { exit_status }) => {
                    exit_code = Some(exit_status);
                }
                Some(ChannelMsg::Eof) => {}
                Some(ChannelMsg::Close) => break,
                Some(_) => {}
                None => break,
            }
        }
    })
    .await;

    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "", "en")
        .await;

    if result.is_err() {
        return Err(eyre!("SSH command timed out after {timeout_secs}s on {host}"));
    }

    Ok(CommandOutput {
        stdout: String::from_utf8_lossy(&stdout).to_string(),
        stderr: String::from_utf8_lossy(&stderr).to_string(),
        exit_code: exit_code.map(|c| c as i32).unwrap_or(-1),
    })
}
