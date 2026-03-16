use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use color_eyre::{eyre::eyre, Result};
use openssh::{KnownHosts, Session, SessionBuilder, Stdio};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use crate::config::HostConfig;

use super::{CommandOutput, RemoteHost};

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

pub struct SshHost {
    session: Option<Arc<Session>>,
    host: String,
    destination: String,
    id: String,
    ctl_path: PathBuf,
    log_path: PathBuf,
    command_count: AtomicU64,
}

impl SshHost {
    pub async fn connect(config: &HostConfig) -> Result<Self> {
        let destination = format!("{}@{}", config.ssh_user, config.address);
        let seq = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
        let id = format!("ssh-{seq}-{}", config.address);

        tracing::info!(id = %id, dest = %destination, "SSH connecting...");

        // ServerAliveInterval is deliberately disabled (0).
        // With -N (no command) on the master, keepalives are checked against
        // the master's own idle connection. If the server is under heavy I/O
        // (e.g., pkg install downloading 600MB), it may not respond to keepalives
        // within ServerAliveInterval × ServerAliveCountMax (default 30s × 3 = 90s),
        // causing the master to kill itself and all child channels.
        // Our 120s execute timeout provides the safety net instead.
        let session = SessionBuilder::default()
            .known_hosts_check(KnownHosts::Accept)
            .connect_timeout(Duration::from_secs(10))
            .connect_mux(&destination)
            .await
            .map_err(|e| eyre!("SSH connection to {destination} failed: {e}"))?;

        let ctl_path = session.control_socket().to_path_buf();
        let log_path = ctl_path
            .parent()
            .map(|p| p.join("log"))
            .unwrap_or_default();

        tracing::info!(
            id = %id,
            dest = %destination,
            ctl = %ctl_path.display(),
            log = %log_path.display(),
            "SSH connected"
        );

        // Register in the global registry
        super::registry::register(&id, &destination, ctl_path.clone(), log_path.clone());

        Ok(Self {
            session: Some(Arc::new(session)),
            host: config.address.clone(),
            destination,
            id,
            ctl_path,
            log_path,
            command_count: AtomicU64::new(0),
        })
    }

    /// Set a label for this session (shown in debug view).
    pub fn set_label(&self, label: &str) {
        super::registry::set_label(&self.id, label);
    }

    /// Check if the mux control socket still exists on disk.
    pub fn socket_exists(&self) -> bool {
        self.ctl_path.exists()
    }

    /// Read the SSH mux master's log file.
    pub fn master_log_content(&self) -> Option<String> {
        std::fs::read_to_string(&self.log_path).ok()
    }

    /// Get the control socket path.
    pub fn ctl_path(&self) -> &PathBuf {
        &self.ctl_path
    }

    /// Get the session ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Pre-command health check. Returns an error if the mux master is dead.
    fn check_health(&self, cmd: &str) -> Result<()> {
        if !self.socket_exists() {
            let log = self.master_log_content().unwrap_or_else(|| "<no log>".to_string());
            tracing::error!(
                id = %self.id,
                host = %self.host,
                ctl = %self.ctl_path.display(),
                "Mux socket MISSING before command: {cmd}"
            );
            tracing::error!(id = %self.id, "Master log:\n{log}");
            return Err(eyre!(
                "SSH mux master for {} died (socket {} missing).\nMaster log:\n{}",
                self.destination,
                self.ctl_path.display(),
                log
            ));
        }
        Ok(())
    }

    fn get_session(&self) -> Result<&Arc<Session>> {
        self.session
            .as_ref()
            .ok_or_else(|| eyre!("SSH session to {} already closed", self.host))
    }

    /// Explicitly close the SSH session and kill the mux master.
    pub async fn close(mut self) -> Result<()> {
        let id = self.id.clone();
        let dest = self.destination.clone();
        super::registry::unregister(&id);

        if let Some(session) = self.session.take() {
            let session = Arc::try_unwrap(session)
                .map_err(|_| eyre!("Cannot close session: outstanding references exist"))?;
            session
                .close()
                .await
                .map_err(|e| eyre!("Failed to close SSH session to {}: {e}", self.host))?;
            tracing::info!(id = %id, dest = %dest, "SSH session closed");
        }
        Ok(())
    }
}

#[async_trait]
impl RemoteHost for SshHost {
    async fn execute(&self, cmd: &str) -> Result<CommandOutput> {
        let cmd_short: String = cmd.chars().take(80).collect();
        let count = self.command_count.fetch_add(1, Ordering::Relaxed) + 1;
        super::registry::record_command(&self.id, &cmd_short);

        tracing::info!(
            id = %self.id,
            host = %self.host,
            cmd_num = count,
            socket_exists = self.socket_exists(),
            cmd = %cmd_short,
            "execute: starting"
        );

        // Fail fast if mux master is dead
        self.check_health(cmd)?;

        let start = Instant::now();
        let session = self.get_session()?.clone();
        let cmd_owned = cmd.to_string();
        let output = tokio::time::timeout(
            Duration::from_secs(120),
            async {
                session
                    .arc_command("sh")
                    .arg("-c")
                    .arg(&cmd_owned)
                    .output()
                    .await
            },
        )
        .await
        .map_err(|_| {
            let socket_exists = self.socket_exists();
            let log = self.master_log_content().unwrap_or_else(|| "<no log file>".to_string());
            tracing::error!(
                id = %self.id,
                host = %self.host,
                socket_exists = socket_exists,
                "execute TIMED OUT after 120s! Master log:\n{log}"
            );
            eyre!(
                "SSH command timed out after 120s on {}. Socket exists: {}.\nMaster log:\n{}",
                self.destination, socket_exists, log
            )
        })?
        .map_err(|e| eyre!("Command failed on {}: {e}", self.host))?;

        let exit_code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let elapsed = start.elapsed();

        tracing::info!(
            id = %self.id,
            host = %self.host,
            cmd_num = count,
            exit_code = exit_code,
            elapsed_ms = elapsed.as_millis() as u64,
            socket_exists = self.socket_exists(),
            "execute: completed"
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

        tracing::info!(
            id = %self.id,
            host = %self.host,
            cmd_num = count,
            socket_exists = self.socket_exists(),
            cmd = %cmd_short,
            "execute_streaming: starting"
        );

        // Fail fast if mux master is dead
        self.check_health(cmd)?;

        let start = Instant::now();
        let mut child = self
            .get_session()?
            .clone()
            .arc_command("sh")
            .arg("-c")
            .arg(cmd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .await
            .map_err(|e| eyre!("Failed to spawn command on {}: {e}", self.host))?;

        tracing::info!(id = %self.id, "execute_streaming: child spawned");

        let stdout = child.stdout().take();
        let stderr = child.stderr().take();

        let stdout_handle = if let Some(stdout) = stdout {
            let tx_clone = tx.clone();
            Some(tokio::spawn(async move {
                let reader = BufReader::new(stdout);
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

        let stderr_handle = if let Some(stderr) = stderr {
            let tx_clone = tx.clone();
            Some(tokio::spawn(async move {
                let reader = BufReader::new(stderr);
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

        // Drop our sender so the channel can close once the reader tasks finish
        drop(tx);

        tracing::info!(id = %self.id, "execute_streaming: waiting for child...");
        let status = child
            .wait()
            .await
            .map_err(|e| eyre!("Failed waiting for command on {}: {e}", self.host))?;

        tracing::info!(
            id = %self.id,
            socket_exists = self.socket_exists(),
            "execute_streaming: child exited, awaiting readers..."
        );

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
            socket_exists = self.socket_exists(),
            "execute_streaming: fully complete"
        );

        Ok(exit_code)
    }

    fn hostname(&self) -> &str {
        &self.host
    }

    async fn check(&self) -> Result<()> {
        self.get_session()?
            .check()
            .await
            .map_err(|e| eyre!("SSH connection check failed for {}: {e}", self.host))?;
        Ok(())
    }
}
