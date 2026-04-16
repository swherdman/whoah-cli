pub mod auth;
pub mod download;
pub mod handler;
pub mod mock;
pub mod oneshot;
pub mod probe;
pub mod registry;
pub mod session;

use async_trait::async_trait;
use color_eyre::Result;
use tokio::sync::mpsc;

#[derive(Debug, Clone)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

#[async_trait]
pub trait RemoteHost: Send + Sync {
    /// Execute a command and collect complete output.
    async fn execute(&self, cmd: &str) -> Result<CommandOutput>;

    /// Execute a command and stream stdout lines through the channel.
    /// Returns exit code when command completes.
    async fn execute_streaming(&self, cmd: &str, tx: mpsc::Sender<String>) -> Result<i32>;

    /// Hostname or address of this host.
    fn hostname(&self) -> &str;

    /// Check if the connection is alive.
    #[allow(dead_code)]
    async fn check(&self) -> Result<()>;
}
