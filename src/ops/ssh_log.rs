//! SSH command logging and execution helpers.
//!
//! Wraps SSH command execution with logging to a file and BuildEvent reporting.

use std::path::PathBuf;

use color_eyre::{Result, eyre::eyre};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

use crate::event::BuildEvent;
use crate::ssh::RemoteHost;

/// A logged SSH session that writes all command I/O to a log file
/// and sends step detail updates to the TUI.
pub struct LoggedSsh<'a> {
    host: &'a dyn RemoteHost,
    log_file: tokio::fs::File,
    log_path: PathBuf,
    tx: mpsc::UnboundedSender<BuildEvent>,
    step_id: String,
    /// Source label written into every log line (e.g. "Watchdog", "Build/Omicron").
    label: String,
    /// Proxy env string, set via `set_proxy()`. Prepended by proxy methods.
    proxy_env: Option<String>,
}

impl<'a> LoggedSsh<'a> {
    /// Create a new logged SSH session.
    ///
    /// `label` is embedded in every log line so concurrent sessions writing to
    /// the same file can be distinguished (e.g. "Watchdog", "Build/Omicron").
    /// Pass the same string you gave to `host.set_label(...)`.
    pub async fn new(
        host: &'a dyn RemoteHost,
        log_path: PathBuf,
        tx: &mpsc::UnboundedSender<BuildEvent>,
        step_id: &str,
        label: &str,
    ) -> Result<Self> {
        if let Some(parent) = log_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }

        let mut log_file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .await
            .map_err(|e| eyre!("Failed to open log {}: {e}", log_path.display()))?;

        let header = format!(
            "--- SSH log for {} label={} step={} at {} ---\n",
            host.hostname(),
            label,
            step_id,
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
        let _ = log_file.write_all(header.as_bytes()).await;

        Ok(Self {
            host,
            log_file,
            log_path,
            tx: tx.clone(),
            step_id: step_id.to_string(),
            label: label.to_string(),
            proxy_env: None,
        })
    }

    /// Switch to logging for a different step (reuses the same log file).
    pub fn set_step(&mut self, step_id: &str) {
        self.step_id = step_id.to_string();
    }

    /// Set proxy environment variables for use with proxy methods.
    pub fn set_proxy(&mut self, proxy_url: &str, ca_cert_path: &str) {
        self.proxy_env = Some(format!(
            "https_proxy={proxy_url} HTTPS_PROXY={proxy_url} \
             SSL_CERT_FILE={ca_cert_path} REQUESTS_CA_BUNDLE={ca_cert_path} \
             no_proxy=localhost,127.0.0.1 NO_PROXY=localhost,127.0.0.1"
        ));
    }

    fn proxy_prefix(&self) -> String {
        match &self.proxy_env {
            Some(env) => format!("{env} "),
            None => String::new(),
        }
    }

    /// Execute a command, log output, send step detail, return output.
    pub async fn run(&mut self, cmd: &str) -> Result<crate::ssh::CommandOutput> {
        self.log_line(&format!(">>> {cmd}")).await;
        self.log_line("    [run] calling host.execute...").await;
        self.detail(cmd).await;

        let output = self.host.execute(cmd).await?;
        self.log_line("    [run] host.execute returned").await;

        // Log stdout
        for line in output.stdout.lines() {
            self.log_line(&format!("    {line}")).await;
        }
        // Log stderr
        for line in output.stderr.lines() {
            self.log_line(&format!("ERR {line}")).await;
        }
        self.log_line(&format!("--- exit_code={}", output.exit_code))
            .await;

        Ok(output)
    }

    /// Execute a command and check for success. Fails the step on non-zero exit.
    pub async fn run_check(&mut self, cmd: &str) -> Result<crate::ssh::CommandOutput> {
        let output = self.run(cmd).await?;
        if output.exit_code != 0 {
            let msg = format!(
                "Command failed (exit {}): {}",
                output.exit_code,
                output.stderr.trim()
            );
            self.fail(&msg).await;
            return Err(eyre!("{msg}"));
        }
        Ok(output)
    }

    /// Execute a command with secrets redacted in the log and TUI output.
    /// The original (un-redacted) command is forwarded to the host unchanged;
    /// each element of `secrets` is replaced with `***` in logged lines and
    /// in the `StepDetail` TUI message. Empty secret strings are silently
    /// ignored.
    pub async fn run_redacted(
        &mut self,
        cmd: &str,
        secrets: &[&str],
    ) -> Result<crate::ssh::CommandOutput> {
        let redact = |s: &str| -> String {
            secrets
                .iter()
                .filter(|x| !x.is_empty())
                .fold(s.to_string(), |acc, sec| acc.replace(sec, "***"))
        };
        let visible = redact(cmd);
        self.log_line(&format!(">>> {visible}")).await;
        self.log_line("    [run] calling host.execute...").await;
        self.detail(&visible).await;
        let output = self.host.execute(cmd).await?;
        self.log_line("    [run] host.execute returned").await;
        for line in output.stdout.lines() {
            self.log_line(&format!("    {}", redact(line))).await;
        }
        for line in output.stderr.lines() {
            self.log_line(&format!("ERR {}", redact(line))).await;
        }
        self.log_line(&format!("--- exit_code={}", output.exit_code))
            .await;
        Ok(output)
    }

    /// Execute a command and log its I/O to the file, but do NOT send the
    /// command or output to the TUI. Use for polling / diagnostic commands
    /// that would otherwise flood the step output buffer.
    pub async fn run_quiet(&mut self, cmd: &str) -> Result<crate::ssh::CommandOutput> {
        self.log_line(&format!(">>> {cmd}")).await;
        self.log_line("    [run] calling host.execute...").await;

        let output = self.host.execute(cmd).await?;
        self.log_line("    [run] host.execute returned").await;

        for line in output.stdout.lines() {
            self.log_line(&format!("    {line}")).await;
        }
        for line in output.stderr.lines() {
            self.log_line(&format!("ERR {line}")).await;
        }
        self.log_line(&format!("--- exit_code={}", output.exit_code))
            .await;

        Ok(output)
    }

    /// Execute a streaming command with proxy env vars, check for success.
    pub async fn run_streaming_check_with_proxy(&mut self, cmd: &str) -> Result<()> {
        let full_cmd = format!("{}{cmd}", self.proxy_prefix());
        self.run_streaming_check(&full_cmd).await
    }

    /// Execute a command via true streaming — lines are forwarded to the log
    /// and TUI as they arrive.
    pub async fn run_streaming(&mut self, cmd: &str) -> Result<i32> {
        self.log_line(&format!(">>> {cmd} (streaming)")).await;
        self.detail(cmd).await;

        let (line_tx, line_rx) = tokio::sync::mpsc::channel::<String>(256);

        // We need to consume lines concurrently with the SSH command running.
        // Copy what we need for the log/detail forwarding task.
        let step_id = self.step_id.clone();
        let tx = self.tx.clone();
        let log_path = self.log_path.clone();
        let label = self.label.clone();

        let forward_handle = tokio::spawn(async move {
            let mut line_rx = line_rx;
            let mut log_file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .await
                .ok();

            let mut count = 0u64;
            while let Some(line) = line_rx.recv().await {
                count += 1;
                // Log to file
                if let Some(ref mut f) = log_file {
                    let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");
                    let entry = format!("[{timestamp}] [{:<16}]     {line}\n", label);
                    let _ = tokio::io::AsyncWriteExt::write_all(f, entry.as_bytes()).await;
                    let _ = tokio::io::AsyncWriteExt::flush(f).await;
                }
                // Send to TUI (filtered)
                let trimmed = line.trim();
                if !trimmed.is_empty() && !is_noise_line(trimmed) {
                    let _ = tx.send(BuildEvent::StepDetail(step_id.clone(), trimmed.to_string()));
                }
            }

            // Log channel close
            if let Some(ref mut f) = log_file {
                let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");
                let entry = format!(
                    "[{timestamp}] [{:<16}]     [forward] channel closed after {count} lines\n",
                    label
                );
                let _ = tokio::io::AsyncWriteExt::write_all(f, entry.as_bytes()).await;
                let _ = tokio::io::AsyncWriteExt::flush(f).await;
            }
        });

        self.log_line("    [streaming] calling execute_streaming...")
            .await;
        let exit_code = self.host.execute_streaming(cmd, line_tx).await?;
        self.log_line(&format!(
            "    [streaming] execute_streaming returned exit={exit_code}"
        ))
        .await;

        self.log_line("    [streaming] awaiting forward_handle...")
            .await;
        let _ = forward_handle.await;
        self.log_line("    [streaming] forward_handle done").await;

        self.log_line(&format!("--- exit_code={exit_code}")).await;
        Ok(exit_code)
    }

    /// Execute a streaming command and check for success.
    pub async fn run_streaming_check(&mut self, cmd: &str) -> Result<()> {
        let exit_code = self.run_streaming(cmd).await?;
        if exit_code != 0 {
            let msg = format!("Command failed with exit code {exit_code}");
            self.fail(&msg).await;
            return Err(eyre!("{msg}"));
        }
        Ok(())
    }

    /// Send a step detail update to the TUI.
    pub async fn detail(&mut self, msg: &str) {
        let _ = self.tx.send(BuildEvent::StepDetail(
            self.step_id.clone(),
            msg.to_string(),
        ));
    }

    /// Mark the current step as failed.
    pub async fn fail(&mut self, msg: &str) {
        self.log_line(&format!("!!! FAILED: {msg}")).await;
        let _ = self.tx.send(BuildEvent::StepFailed(
            self.step_id.clone(),
            msg.to_string(),
        ));
    }

    async fn log_line(&mut self, line: &str) {
        let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");
        let entry = format!("[{timestamp}] [{:<16}] {line}\n", self.label);
        let _ = self.log_file.write_all(entry.as_bytes()).await;
        let _ = self.log_file.flush().await;
    }
}

/// Returns true if a streaming output line is noise that shouldn't be
/// sent to the TUI. The line is still logged to the build log file.
fn is_noise_line(line: &str) -> bool {
    // SSH warnings
    if line.starts_with("Warning: Permanently added") {
        return true;
    }
    // Shell env setup noise (from `source env.sh` with xtrace)
    if line.starts_with("++ export PATH=") {
        return true;
    }
    if line.starts_with("++ PATH=") {
        return true;
    }
    if line.starts_with("++ set +o xtrace") {
        return true;
    }
    if line.starts_with("++ unset ") {
        return true;
    }
    if line.starts_with("++ case ") {
        return true;
    }
    if line.starts_with("++++ dirname ") {
        return true;
    }
    if line.starts_with("+++ readlink ") {
        return true;
    }
    if line.starts_with("++ OMICRON_WS=") {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::mock::MockHost;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn test_run_redacted_hides_secret_in_log() {
        let mut mock = MockHost::new("test-host");
        mock.add_success("SECRET_CMD", "cmd output line");

        let (tx, _rx) = mpsc::unbounded_channel();
        let log_path = std::env::temp_dir().join("whoah-redacted-test.log");
        let _ = tokio::fs::remove_file(&log_path).await;

        let mut ssh = LoggedSsh::new(&mock, log_path.clone(), &tx, "test-step", "TestRedacted")
            .await
            .expect("LoggedSsh::new failed");

        let result = ssh
            .run_redacted("SECRET_CMD", &["SECRET"])
            .await
            .expect("run_redacted failed");

        assert_eq!(result.exit_code, 0);
        assert_eq!(result.stdout.trim(), "cmd output line");

        // Verify log file contains *** and not the secret
        let log_contents = tokio::fs::read_to_string(&log_path)
            .await
            .expect("read log");
        assert!(
            !log_contents.contains("SECRET_CMD"),
            "log must not contain original command"
        );
        assert!(
            log_contents.contains("***_CMD"),
            "log must contain redacted command"
        );
        // Output should also be redacted (no SECRET in stdout lines)
        let stdout_lines: Vec<&str> = log_contents
            .lines()
            .filter(|l| l.contains("cmd output line"))
            .collect();
        assert!(!stdout_lines.is_empty(), "stdout line must appear in log");
    }
}
