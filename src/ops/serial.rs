//! Serial console interaction via socat over SSH.
//!
//! Opens a persistent bidirectional connection to a Proxmox VM's serial console
//! through its unix socket. Commands are written to stdin, output is read
//! asynchronously as it arrives.

use std::path::PathBuf;
use std::sync::Arc;

use color_eyre::{eyre::eyre, Result};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, WriteHalf};
use tokio::sync::mpsc;

use crate::ssh::auth::authenticate;
use crate::ssh::handler::SshClientHandler;

/// A live serial console connection to a Proxmox VM.
pub struct SerialConsole {
    writer: WriteHalf<russh::ChannelStream<russh::client::Msg>>,
    lines_rx: mpsc::UnboundedReceiver<String>,
    log_file: Option<tokio::fs::File>,
    _handle: Arc<russh::client::Handle<SshClientHandler>>,
}

impl SerialConsole {
    /// Open a serial console connection to a VM via SSH to the Proxmox host.
    /// If `log_path` is provided, all console I/O is logged to that file.
    pub async fn connect(
        proxmox_host: &str,
        proxmox_user: &str,
        proxmox_port: u16,
        vmid: u32,
    ) -> Result<Self> {
        Self::connect_with_log(proxmox_host, proxmox_user, proxmox_port, vmid, None).await
    }

    /// Open a serial console with logging to a file.
    pub async fn connect_with_log(
        proxmox_host: &str,
        proxmox_user: &str,
        proxmox_port: u16,
        vmid: u32,
        log_path: Option<PathBuf>,
    ) -> Result<Self> {
        let destination = format!("{proxmox_user}@{proxmox_host}");
        let config = Arc::new(russh::client::Config {
            keepalive_interval: Some(Duration::from_secs(30)),
            keepalive_max: 6,
            ..Default::default()
        });
        let handler = SshClientHandler::new();

        let mut handle = tokio::time::timeout(
            Duration::from_secs(10),
            russh::client::connect(config, format!("{proxmox_host}:{proxmox_port}"), handler),
        )
        .await
        .map_err(|_| eyre!("SSH to Proxmox host {destination} timed out"))?
        .map_err(|e| eyre!("SSH to Proxmox host {destination} failed: {e}"))?;

        authenticate(&mut handle, proxmox_user)
            .await
            .map_err(|e| eyre!("SSH auth to {destination} failed: {e}"))?;

        let handle = Arc::new(handle);

        let socket_path = format!("/var/run/qemu-server/{vmid}.serial0");
        let socat_cmd = format!("socat - UNIX-CONNECT:{socket_path}");

        let channel = handle
            .channel_open_session()
            .await
            .map_err(|e| eyre!("Failed to open channel for serial console: {e}"))?;

        channel
            .exec(true, socat_cmd.as_bytes())
            .await
            .map_err(|e| eyre!("Failed to spawn socat for serial console: {e}"))?;

        // Convert the channel to AsyncRead + AsyncWrite stream, then split
        let stream = channel.into_stream();
        let (reader, writer) = tokio::io::split(stream);

        // Open log file if requested
        let log_file = if let Some(ref path) = log_path {
            if let Some(parent) = path.parent() {
                let _ = tokio::fs::create_dir_all(parent).await;
            }
            let mut f = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .await
                .map_err(|e| eyre!("Failed to open serial log {}: {e}", path.display()))?;
            let header = format!(
                "--- Serial console log for VMID {vmid} at {} ---\n",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
            );
            let _ = f.write_all(header.as_bytes()).await;
            Some(f)
        } else {
            None
        };

        // Spawn a reader task that reads chunks and splits on newlines.
        // Flushes partial lines (like "login: " prompts) after 500ms of quiet.
        let (lines_tx, lines_rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            let mut buf = vec![0u8; 4096];
            let mut partial = String::new();

            loop {
                tokio::select! {
                    result = reader.read(&mut buf) => {
                        match result {
                            Ok(0) | Err(_) => {
                                // EOF or error — flush any remaining partial line
                                if !partial.is_empty() {
                                    let clean = strip_ansi_and_cr(&partial);
                                    if !clean.is_empty() {
                                        let _ = lines_tx.send(clean);
                                    }
                                }
                                break;
                            }
                            Ok(n) => {
                                let chunk = String::from_utf8_lossy(&buf[..n]);
                                partial.push_str(&chunk);

                                // Split on newlines, keeping any trailing partial
                                while let Some(pos) = partial.find('\n') {
                                    let line = partial[..pos].to_string();
                                    partial = partial[pos + 1..].to_string();
                                    let clean = strip_ansi_and_cr(&line);
                                    if !clean.is_empty() {
                                        let _ = lines_tx.send(clean);
                                    }
                                }
                            }
                        }
                    }
                    // Flush partial line after 500ms of no new data
                    // (catches prompts like "login: " that don't end with \n)
                    _ = tokio::time::sleep(Duration::from_millis(500)), if !partial.is_empty() => {
                        let clean = strip_ansi_and_cr(&partial);
                        partial.clear();
                        if !clean.is_empty() {
                            let _ = lines_tx.send(clean);
                        }
                    }
                }
            }
        });

        Ok(Self {
            writer,
            lines_rx,
            log_file,
            _handle: handle,
        })
    }

    /// Write a line to the log file (if logging is enabled).
    async fn log(&mut self, prefix: &str, text: &str) {
        if let Some(ref mut f) = self.log_file {
            let timestamp = chrono::Local::now().format("%H:%M:%S%.3f");
            let line = format!("[{timestamp}] {prefix} {text}\n");
            let _ = f.write_all(line.as_bytes()).await;
            let _ = f.flush().await;
        }
    }

    /// Send a command to the serial console (appends \r\n).
    pub async fn send(&mut self, cmd: &str) -> Result<()> {
        self.log(">>>", cmd).await;
        self.writer
            .write_all(format!("{cmd}\r\n").as_bytes())
            .await
            .map_err(|e| eyre!("Failed to write to serial console: {e}"))?;
        self.writer
            .flush()
            .await
            .map_err(|e| eyre!("Failed to flush serial console: {e}"))?;
        Ok(())
    }

    /// Send a bare string without appending a newline (for raw input like \r).
    pub async fn send_raw(&mut self, data: &str) -> Result<()> {
        let display = data.replace('\r', "\\r").replace('\n', "\\n");
        self.log(">>>", &format!("(raw) {display}")).await;
        self.writer
            .write_all(data.as_bytes())
            .await
            .map_err(|e| eyre!("Failed to write to serial console: {e}"))?;
        self.writer
            .flush()
            .await
            .map_err(|e| eyre!("Failed to flush serial console: {e}"))?;
        Ok(())
    }

    /// Read the next line from the serial console.
    /// Returns None if the connection is closed.
    pub async fn recv(&mut self) -> Option<String> {
        let line = self.lines_rx.recv().await;
        if let Some(ref l) = line {
            self.log("<<<", l).await;
        }
        line
    }

    /// Read lines until one matches the predicate, with a timeout.
    /// Returns the matching line, or an error on timeout.
    /// All lines (including non-matching) are forwarded to `on_line` if provided.
    pub async fn wait_for<F>(
        &mut self,
        timeout: Duration,
        mut on_line: impl FnMut(&str),
        predicate: F,
    ) -> Result<String>
    where
        F: Fn(&str) -> bool,
    {
        let deadline = tokio::time::Instant::now() + timeout;

        loop {
            tokio::select! {
                line = self.lines_rx.recv() => {
                    match line {
                        Some(line) => {
                            self.log("<<<", &line).await;
                            on_line(&line);
                            if predicate(&line) {
                                self.log("---", "^ matched predicate").await;
                                return Ok(line);
                            }
                        }
                        None => {
                            self.log("!!!", "Serial console connection closed").await;
                            return Err(eyre!("Serial console connection closed"));
                        }
                    }
                }
                _ = tokio::time::sleep_until(deadline) => {
                    self.log("!!!", &format!("Timeout after {}s", timeout.as_secs())).await;
                    return Err(eyre!("Timeout waiting for serial console output"));
                }
            }
        }
    }

    /// Wait for a shell prompt, periodically sending CR to nudge the console.
    /// Useful after boot when the shell may not be ready yet.
    pub async fn wait_for_prompt(
        &mut self,
        timeout: Duration,
        nudge_interval: Duration,
        mut on_line: impl FnMut(&str),
    ) -> Result<String> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut nudge_interval = tokio::time::interval(nudge_interval);
        nudge_interval.tick().await; // consume the immediate first tick

        loop {
            tokio::select! {
                line = self.lines_rx.recv() => {
                    match line {
                        Some(line) => {
                            self.log("<<<", &line).await;
                            on_line(&line);
                            if line.trim().ends_with('#') {
                                self.log("---", "^ matched prompt (#)").await;
                                return Ok(line);
                            }
                        }
                        None => {
                            self.log("!!!", "Serial console connection closed").await;
                            return Err(eyre!("Serial console connection closed"));
                        }
                    }
                }
                _ = nudge_interval.tick() => {
                    self.log("---", "nudge CR").await;
                    let _ = self.send_raw("\r").await;
                }
                _ = tokio::time::sleep_until(deadline) => {
                    self.log("!!!", &format!("Timeout after {}s waiting for prompt", timeout.as_secs())).await;
                    return Err(eyre!("Timeout waiting for shell prompt"));
                }
            }
        }
    }

    /// Read all available lines until no more arrive for `quiet_duration`.
    /// Useful for draining output after a command.
    pub async fn drain(&mut self, quiet_duration: Duration, mut on_line: impl FnMut(&str)) {
        loop {
            tokio::select! {
                line = self.lines_rx.recv() => {
                    match line {
                        Some(line) => on_line(&line),
                        None => return,
                    }
                }
                _ = tokio::time::sleep(quiet_duration) => {
                    return;
                }
            }
        }
    }
}

/// Strip ANSI escape sequences and carriage returns from a line.
fn strip_ansi_and_cr(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip ESC [ ... (letter) sequences
            if chars.peek() == Some(&'[') {
                chars.next(); // consume '['
                // Consume parameter bytes (0x30-0x3F), intermediate bytes (0x20-0x2F),
                // and final byte (0x40-0x7E)
                loop {
                    match chars.peek() {
                        Some(&c) if (0x40 as char..=0x7E as char).contains(&c) => {
                            chars.next(); // consume final byte
                            break;
                        }
                        Some(_) => {
                            chars.next();
                        }
                        None => break,
                    }
                }
            }
        } else if c == '\r' {
            // Skip carriage returns
        } else {
            result.push(c);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_simple() {
        assert_eq!(strip_ansi_and_cr("\x1b[?2004l# "), "# ");
    }

    #[test]
    fn test_strip_ansi_with_cr() {
        assert_eq!(
            strip_ansi_and_cr("\x1b[?2004l\rSunOS unknown 5.11\r"),
            "SunOS unknown 5.11"
        );
    }

    #[test]
    fn test_strip_plain_text() {
        assert_eq!(strip_ansi_and_cr("hello world"), "hello world");
    }

    #[test]
    fn test_strip_color_codes() {
        assert_eq!(
            strip_ansi_and_cr("\x1b[32mgreen\x1b[0m text"),
            "green text"
        );
    }
}
