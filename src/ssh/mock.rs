use std::collections::HashMap;

use async_trait::async_trait;
use color_eyre::{Result, eyre::eyre};
use tokio::sync::mpsc;

use super::{CommandOutput, RemoteHost};

/// Mock SSH host for testing. Matches commands by substring.
pub struct MockHost {
    hostname: String,
    responses: HashMap<String, CommandOutput>,
}

impl MockHost {
    pub fn new(hostname: &str) -> Self {
        Self {
            hostname: hostname.to_string(),
            responses: HashMap::new(),
        }
    }

    /// Add a canned response. The pattern is matched as a substring of the command.
    pub fn add_response(&mut self, cmd_pattern: &str, output: CommandOutput) {
        self.responses.insert(cmd_pattern.to_string(), output);
    }

    /// Convenience: add a successful response with given stdout.
    pub fn add_success(&mut self, cmd_pattern: &str, stdout: &str) {
        self.responses.insert(
            cmd_pattern.to_string(),
            CommandOutput {
                stdout: stdout.to_string(),
                stderr: String::new(),
                exit_code: 0,
            },
        );
    }

    /// Convenience: add a failed response.
    pub fn add_failure(&mut self, cmd_pattern: &str, stderr: &str, exit_code: i32) {
        self.responses.insert(
            cmd_pattern.to_string(),
            CommandOutput {
                stdout: String::new(),
                stderr: stderr.to_string(),
                exit_code,
            },
        );
    }

    fn find_response(&self, cmd: &str) -> Option<&CommandOutput> {
        // Try exact match first, then substring
        if let Some(resp) = self.responses.get(cmd) {
            return Some(resp);
        }
        for (pattern, resp) in &self.responses {
            if cmd.contains(pattern.as_str()) {
                return Some(resp);
            }
        }
        None
    }
}

#[async_trait]
impl RemoteHost for MockHost {
    async fn execute(&self, cmd: &str) -> Result<CommandOutput> {
        match self.find_response(cmd) {
            Some(resp) => Ok(resp.clone()),
            None => Err(eyre!("MockHost: no response configured for command: {cmd}")),
        }
    }

    async fn execute_streaming(&self, cmd: &str, tx: mpsc::Sender<String>) -> Result<i32> {
        match self.find_response(cmd) {
            Some(resp) => {
                for line in resp.stdout.lines() {
                    let _ = tx.send(line.to_string()).await;
                }
                Ok(resp.exit_code)
            }
            None => Err(eyre!("MockHost: no response configured for command: {cmd}")),
        }
    }

    fn hostname(&self) -> &str {
        &self.hostname
    }

    async fn check(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_execute() {
        let mut mock = MockHost::new("test-host");
        mock.add_success(
            "zpool list",
            "rpool\t100\t50\t50\t-\t-\t0\t50\t1.00\tONLINE\t-\n",
        );

        let result = mock.execute("zpool list -Hp").await.unwrap();
        assert_eq!(result.exit_code, 0);
        assert!(result.stdout.contains("rpool"));
    }

    #[tokio::test]
    async fn test_mock_no_match() {
        let mock = MockHost::new("test-host");
        let result = mock.execute("unknown command").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_mock_streaming() {
        let mut mock = MockHost::new("test-host");
        mock.add_success("build", "line1\nline2\nline3\n");

        let (tx, mut rx) = mpsc::channel(32);
        let exit_code = mock.execute_streaming("cargo build", tx).await.unwrap();
        assert_eq!(exit_code, 0);

        let mut lines = Vec::new();
        while let Ok(line) = rx.try_recv() {
            lines.push(line);
        }
        assert_eq!(lines, vec!["line1", "line2", "line3"]);
    }
}
