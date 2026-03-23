//! SSH session using russh.
//!
//! Implements the RemoteHost trait using russh's pure-Rust SSH2 protocol.
//! Each connection is a single TCP socket with native SSH2 channel
//! multiplexing — no ControlMaster, no mux sockets, no external processes.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use color_eyre::{eyre::eyre, Result};
use russh::client::Handle;
use russh::ChannelMsg;
use russh::Disconnect;
use tokio::sync::mpsc;

use crate::config::HostConfig;

use super::auth::authenticate;
use super::handler::SshClientHandler;
use super::{CommandOutput, RemoteHost};

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct SshHost {
    handle: Arc<Handle<SshClientHandler>>,
    host: String,
    destination: String,
    id: String,
    command_count: AtomicU64,
}

impl std::fmt::Debug for SshHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshHost")
            .field("host", &self.host)
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

impl SshHost {
    pub async fn connect(config: &HostConfig) -> Result<Self> {
        let destination = format!("{}@{}", config.ssh_user, config.address);
        let seq = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let id = format!("ssh-{seq}-{}", config.address);

        tracing::info!(id = %id, dest = %destination, "SSH connecting...");

        let russh_config = Arc::new(russh::client::Config {
            // Detect dead connections — without keepalives, a TCP connection
            // killed by an intermediate firewall goes unnoticed indefinitely.
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 6, // 30s * 6 = 180s before declaring dead
            ..Default::default()
        });

        let addr = format!("{}:22", config.address);
        let handler = SshClientHandler::new();

        let mut handle = tokio::time::timeout(
            Duration::from_secs(10),
            russh::client::connect(russh_config, &addr, handler),
        )
        .await
        .map_err(|_| eyre!("SSH connection to {destination} timed out after 10s"))?
        .map_err(|e| eyre!("SSH connection to {destination} failed: {e}"))?;

        authenticate(&mut handle, &config.ssh_user)
            .await
            .map_err(|e| eyre!("SSH auth to {destination} failed: {e}"))?;

        let handle = Arc::new(handle);

        tracing::info!(id = %id, dest = %destination, "SSH connected");

        // Register in the global registry
        super::registry::register(&id, &destination);

        Ok(Self {
            handle,
            host: config.address.clone(),
            destination,
            id,
            command_count: AtomicU64::new(0),
        })
    }

    /// Set a label for this session (shown in debug view).
    pub fn set_label(&self, label: &str) {
        super::registry::set_label(&self.id, label);
    }

    /// Check if the SSH connection is still alive.
    pub fn is_connected(&self) -> bool {
        !self.handle.is_closed()
    }

    /// Explicitly close the SSH session.
    pub async fn close(&self) -> Result<()> {
        let id = self.id.clone();
        let dest = self.destination.clone();

        let _ = self
            .handle
            .disconnect(Disconnect::ByApplication, "closing", "en")
            .await;

        super::registry::unregister(&id);
        tracing::info!(id = %id, dest = %dest, "SSH session closed");
        Ok(())
    }
}

impl Drop for SshHost {
    fn drop(&mut self) {
        // Safety net — unregister is idempotent, so this is harmless
        // if close() was already called.
        super::registry::unregister(&self.id);
    }
}

#[async_trait]
impl RemoteHost for SshHost {
    async fn execute(&self, cmd: &str) -> Result<CommandOutput> {
        let cmd_short: String = cmd.chars().take(80).collect();
        let count = self.command_count.fetch_add(1, Ordering::Relaxed) + 1;
        super::registry::record_command(&self.id, &cmd_short);

        tracing::debug!(
            id = %self.id,
            host = %self.host,
            cmd_num = count,
            cmd = %cmd_short,
            "execute: starting"
        );

        let start = std::time::Instant::now();

        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| eyre!("Failed to open SSH channel on {}: {e}", self.host))?;

        channel
            .exec(true, cmd.as_bytes())
            .await
            .map_err(|e| eyre!("Failed to exec on {}: {e}", self.host))?;

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut exit_code: Option<u32> = None;

        let result = tokio::time::timeout(Duration::from_secs(120), async {
            loop {
                match channel.wait().await {
                    Some(ChannelMsg::Data { data }) => {
                        stdout.extend_from_slice(&data);
                    }
                    Some(ChannelMsg::ExtendedData { data, ext }) if ext == 1 => {
                        stderr.extend_from_slice(&data);
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

        if result.is_err() {
            tracing::error!(
                id = %self.id,
                host = %self.host,
                "execute TIMED OUT after 120s"
            );
            return Err(eyre!(
                "SSH command timed out after 120s on {}",
                self.destination
            ));
        }

        let exit = exit_code.map(|c| c as i32).unwrap_or(-1);
        let elapsed = start.elapsed();

        tracing::debug!(
            id = %self.id,
            host = %self.host,
            cmd_num = count,
            exit_code = exit,
            elapsed_ms = elapsed.as_millis() as u64,
            "execute: completed"
        );

        Ok(CommandOutput {
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
            exit_code: exit,
        })
    }

    async fn execute_streaming(
        &self,
        cmd: &str,
        tx: mpsc::Sender<String>,
    ) -> Result<i32> {
        let cmd_short: String = cmd.chars().take(80).collect();
        let count = self.command_count.fetch_add(1, Ordering::Relaxed) + 1;
        super::registry::record_command(&self.id, &cmd_short);

        tracing::debug!(
            id = %self.id,
            host = %self.host,
            cmd_num = count,
            cmd = %cmd_short,
            "execute_streaming: starting"
        );

        let start = std::time::Instant::now();

        let mut channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| eyre!("Failed to open SSH channel on {}: {e}", self.host))?;

        channel
            .exec(true, cmd.as_bytes())
            .await
            .map_err(|e| eyre!("Failed to exec on {}: {e}", self.host))?;

        tracing::info!(id = %self.id, "execute_streaming: channel opened");

        let mut exit_code: Option<u32> = None;
        let mut partial_line = String::new();

        loop {
            match channel.wait().await {
                Some(ChannelMsg::Data { data }) => {
                    let chunk = String::from_utf8_lossy(&data);
                    partial_line.push_str(&chunk);

                    while let Some(pos) = partial_line.find('\n') {
                        let line = partial_line[..pos].to_string();
                        partial_line = partial_line[pos + 1..].to_string();
                        if tx.send(line).await.is_err() {
                            break;
                        }
                    }
                }
                Some(ChannelMsg::ExtendedData { data, ext }) if ext == 1 => {
                    // Stream stderr too (same as current behavior)
                    let chunk = String::from_utf8_lossy(&data);
                    partial_line.push_str(&chunk);

                    while let Some(pos) = partial_line.find('\n') {
                        let line = partial_line[..pos].to_string();
                        partial_line = partial_line[pos + 1..].to_string();
                        if tx.send(line).await.is_err() {
                            break;
                        }
                    }
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

        // Flush any remaining partial line
        if !partial_line.is_empty() {
            let _ = tx.send(partial_line).await;
        }

        let exit = exit_code.map(|c| c as i32).unwrap_or(-1);
        let elapsed = start.elapsed();

        tracing::info!(
            id = %self.id,
            host = %self.host,
            cmd_num = count,
            exit_code = exit,
            elapsed_ms = elapsed.as_millis() as u64,
            "execute_streaming: completed"
        );

        Ok(exit)
    }

    fn hostname(&self) -> &str {
        &self.host
    }

    async fn check(&self) -> Result<()> {
        if self.handle.is_closed() {
            return Err(eyre!("SSH connection to {} is closed", self.host));
        }

        // Open and immediately close a channel to verify the connection works
        let channel = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| eyre!("SSH connection check failed for {}: {e}", self.host))?;
        let _ = channel.close().await;
        Ok(())
    }
}
