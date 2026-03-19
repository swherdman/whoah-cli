//! Direct SSH execution via tokio::process::Command.
//!
//! Uses the system `ssh` binary with OS-level ControlMaster for connection
//! reuse. This avoids the openssh crate's native-mux implementation which
//! has a bug where it refuses new channels after streaming commands in
//! certain runtime contexts (see docs/BUG-ssh-mux-channel-refusal.md).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use async_trait::async_trait;
use color_eyre::{eyre::eyre, Result};
use tokio::io::AsyncBufReadExt;
use tokio::sync::mpsc;

use crate::config::HostConfig;

use super::{CommandOutput, RemoteHost};

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1000);

pub struct DirectSsh {
    user: String,
    host: String,
    id: String,
    ctl_path: PathBuf,
    command_count: AtomicU64,
}

impl DirectSsh {
    /// Connect to a remote host using direct SSH with OS-level ControlMaster.
    pub async fn connect(config: &HostConfig) -> Result<Self> {
        let seq = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let id = format!("dssh-{seq}-{}", config.address);

        // Create a unique control socket path
        let ctl_path = std::env::temp_dir().join(format!(
            "whoah-ssh-{}-{}@{}",
            seq, config.ssh_user, config.address
        ));

        tracing::info!(
            id = %id,
            dest = %format!("{}@{}", config.ssh_user, config.address),
            ctl = %ctl_path.display(),
            "DirectSsh: establishing ControlMaster"
        );

        // Launch the ControlMaster in the background
        let output = tokio::process::Command::new("ssh")
            .args([
                "-o", "StrictHostKeyChecking=no",
                "-o", "UserKnownHostsFile=/dev/null",
                "-o", "ConnectTimeout=10",
                "-o", "ServerAliveInterval=30",
                "-o", "ServerAliveCountMax=6",
                "-o", &format!("ControlPath={}", ctl_path.display()),
                "-o", "ControlMaster=yes",
                "-o", "ControlPersist=300",
                "-M", "-f", "-N",
                &format!("{}@{}", config.ssh_user, config.address),
            ])
            .output()
            .await
            .map_err(|e| eyre!("Failed to launch SSH ControlMaster: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(eyre!(
                "SSH ControlMaster failed (exit {}): {}",
                output.status.code().unwrap_or(-1),
                stderr.trim()
            ));
        }

        // Verify the socket was created
        if !ctl_path.exists() {
            return Err(eyre!(
                "SSH ControlMaster started but socket not found at {}",
                ctl_path.display()
            ));
        }

        // Log the host key fingerprint
        let hostkey = tokio::process::Command::new("ssh-keygen")
            .args(["-l", "-F", &config.address])
            .output()
            .await
            .ok()
            .and_then(|o| {
                let stdout = String::from_utf8_lossy(&o.stdout);
                stdout.lines()
                    .find(|l| l.contains("SHA256:"))
                    .map(|l| l.trim().to_string())
            });
        if let Some(ref key) = hostkey {
            tracing::info!(id = %id, host = %config.address, "DirectSsh: connected — host key: {key}");
        } else {
            tracing::info!(id = %id, host = %config.address, "DirectSsh: connected");
        }

        // Register in the session registry
        super::registry::register(
            &id,
            &format!("{}@{}", config.ssh_user, config.address),
            ctl_path.clone(),
            PathBuf::new(), // no separate log file for direct SSH
        );

        Ok(Self {
            user: config.ssh_user.clone(),
            host: config.address.clone(),
            id,
            ctl_path,
            command_count: AtomicU64::new(0),
        })
    }

    /// Set a label for this session (shown in debug view).
    pub fn set_label(&self, label: &str) {
        super::registry::set_label(&self.id, label);
    }

    fn ssh_args(&self) -> Vec<String> {
        vec![
            "-o".to_string(),
            "StrictHostKeyChecking=no".to_string(),
            "-o".to_string(),
            "UserKnownHostsFile=/dev/null".to_string(),
            "-o".to_string(),
            format!("ControlPath={}", self.ctl_path.display()),
            "-o".to_string(),
            "ControlMaster=auto".to_string(),
            format!("{}@{}", self.user, self.host),
        ]
    }

    /// Explicitly close the SSH session.
    pub async fn close(self) -> Result<()> {
        super::registry::unregister(&self.id);
        let _ = tokio::process::Command::new("ssh")
            .args([
                "-o", &format!("ControlPath={}", self.ctl_path.display()),
                "-O", "exit",
                &format!("{}@{}", self.user, self.host),
            ])
            .output()
            .await;
        tracing::info!(id = %self.id, "DirectSsh: closed");
        Ok(())
    }
}

impl Drop for DirectSsh {
    fn drop(&mut self) {
        super::registry::unregister(&self.id);
        // Best-effort cleanup — send exit to ControlMaster
        let _ = std::process::Command::new("ssh")
            .args([
                "-o", &format!("ControlPath={}", self.ctl_path.display()),
                "-O", "exit",
                &format!("{}@{}", self.user, self.host),
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        tracing::info!(id = %self.id, "DirectSsh: dropped");
    }
}

#[async_trait]
impl RemoteHost for DirectSsh {
    async fn execute(&self, cmd: &str) -> Result<CommandOutput> {
        let cmd_short: String = cmd.chars().take(80).collect();
        let count = self.command_count.fetch_add(1, Ordering::Relaxed) + 1;
        super::registry::record_command(&self.id, &cmd_short);

        tracing::debug!(
            id = %self.id,
            host = %self.host,
            cmd_num = count,
            cmd = %cmd_short,
            "DirectSsh execute: starting"
        );

        let start = Instant::now();
        let mut args = self.ssh_args();
        args.push("--".to_string());
        args.push(cmd.to_string());

        let output = tokio::process::Command::new("ssh")
            .args(&args)
            .output()
            .await
            .map_err(|e| eyre!("SSH command failed on {}: {e}", self.host))?;

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let elapsed = start.elapsed();

        tracing::debug!(
            id = %self.id,
            host = %self.host,
            cmd_num = count,
            exit_code = exit_code,
            elapsed_ms = elapsed.as_millis() as u64,
            "DirectSsh execute: completed"
        );

        Ok(CommandOutput {
            stdout,
            stderr,
            exit_code,
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
            "DirectSsh execute_streaming: starting"
        );

        let start = Instant::now();
        let mut args = self.ssh_args();
        args.push("--".to_string());
        args.push(cmd.to_string());

        let mut child = tokio::process::Command::new("ssh")
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| eyre!("Failed to spawn SSH on {}: {e}", self.host))?;

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        // Stream stdout
        let stdout_handle = if let Some(stdout) = stdout {
            let tx_clone = tx.clone();
            Some(tokio::spawn(async move {
                let reader = tokio::io::BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if tx_clone.send(line).await.is_err() {
                        break;
                    }
                }
            }))
        } else {
            None
        };

        // Stream stderr
        let stderr_handle = if let Some(stderr) = stderr {
            let tx_clone = tx.clone();
            Some(tokio::spawn(async move {
                let reader = tokio::io::BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if tx_clone.send(line).await.is_err() {
                        break;
                    }
                }
            }))
        } else {
            None
        };

        drop(tx);

        let status = child
            .wait()
            .await
            .map_err(|e| eyre!("Failed waiting for SSH on {}: {e}", self.host))?;

        if let Some(h) = stdout_handle {
            let _ = h.await;
        }
        if let Some(h) = stderr_handle {
            let _ = h.await;
        }

        let exit_code = status.code().unwrap_or(-1);
        let elapsed = start.elapsed();

        tracing::info!(
            id = %self.id,
            host = %self.host,
            cmd_num = count,
            exit_code = exit_code,
            elapsed_ms = elapsed.as_millis() as u64,
            "DirectSsh execute_streaming: completed"
        );

        Ok(exit_code)
    }

    fn hostname(&self) -> &str {
        &self.host
    }

    async fn check(&self) -> Result<()> {
        let output = tokio::process::Command::new("ssh")
            .args([
                "-o", &format!("ControlPath={}", self.ctl_path.display()),
                "-O", "check",
                &format!("{}@{}", self.user, self.host),
            ])
            .output()
            .await
            .map_err(|e| eyre!("SSH check failed: {e}"))?;

        if !output.status.success() {
            return Err(eyre!("SSH connection check failed for {}", self.host));
        }
        Ok(())
    }
}
