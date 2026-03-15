use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use color_eyre::{eyre::eyre, Result};
use openssh::{KnownHosts, Session, SessionBuilder, Stdio};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use crate::config::HostConfig;

use super::{CommandOutput, RemoteHost};

pub struct SshHost {
    session: Arc<Session>,
    host: String,
}

impl SshHost {
    pub async fn connect(config: &HostConfig) -> Result<Self> {
        let destination = format!("{}@{}", config.ssh_user, config.address);

        tracing::info!("Connecting to {destination}...");

        let session = SessionBuilder::default()
            .known_hosts_check(KnownHosts::Accept)
            .connect_timeout(Duration::from_secs(10))
            .server_alive_interval(Duration::from_secs(30))
            .clean_history_control_directory(true)
            .connect(&destination)
            .await
            .map_err(|e| eyre!("SSH connection to {destination} failed: {e}"))?;

        tracing::info!("Connected to {destination}");

        Ok(Self {
            session: Arc::new(session),
            host: config.address.clone(),
        })
    }

    /// Explicitly close the SSH session.
    /// Always prefer this over letting the session drop.
    pub async fn close(self) -> Result<()> {
        let session = Arc::try_unwrap(self.session)
            .map_err(|_| eyre!("Cannot close session: outstanding references exist"))?;
        session
            .close()
            .await
            .map_err(|e| eyre!("Failed to close SSH session to {}: {e}", self.host))?;
        Ok(())
    }
}

#[async_trait]
impl RemoteHost for SshHost {
    async fn execute(&self, cmd: &str) -> Result<CommandOutput> {
        tracing::debug!(host = %self.host, cmd = cmd, "Executing command");

        // Use arc_command for safe concurrent access via Arc<Session>
        let output = self
            .session
            .clone()
            .arc_command("sh").arg("-c").arg(cmd)
            .output()
            .await
            .map_err(|e| eyre!("Command failed on {}: {e}", self.host))?;

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();

        tracing::debug!(
            host = %self.host,
            exit_code = exit_code,
            stdout_len = stdout.len(),
            "Command completed"
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
        tracing::debug!(host = %self.host, cmd = cmd, "Executing streaming command");

        let mut child = self
            .session
            .clone()
            .arc_command("sh").arg("-c").arg(cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .await
            .map_err(|e| eyre!("Failed to spawn command on {}: {e}", self.host))?;

        let stdout = child.stdout().take();
        let stderr = child.stderr().take();

        // Stream stdout
        if let Some(stdout) = stdout {
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if tx_clone.send(line).await.is_err() {
                        break;
                    }
                }
            });
        }

        // Stream stderr (merged into same channel)
        if let Some(stderr) = stderr {
            let tx_clone = tx.clone();
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if tx_clone.send(line).await.is_err() {
                        break;
                    }
                }
            });
        }

        let status = child
            .wait()
            .await
            .map_err(|e| eyre!("Failed waiting for command on {}: {e}", self.host))?;

        let exit_code = status.code().unwrap_or(-1);
        tracing::debug!(host = %self.host, exit_code = exit_code, "Streaming command completed");

        Ok(exit_code)
    }

    fn hostname(&self) -> &str {
        &self.host
    }

    async fn check(&self) -> Result<()> {
        self.session
            .check()
            .await
            .map_err(|e| eyre!("SSH connection check failed for {}: {e}", self.host))?;
        Ok(())
    }
}
