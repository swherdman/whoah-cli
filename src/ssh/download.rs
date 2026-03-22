//! Remote file download via SSH + curl with streaming progress.
//!
//! Runs `curl --progress-bar` on a remote host via a one-shot SSH
//! connection (no mux session), parses stderr for progress updates,
//! and streams them back via an mpsc channel.

use color_eyre::{eyre::eyre, Result};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

/// Download progress update.
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    /// Percentage complete (0.0 - 100.0).
    pub percent: f32,
}

/// Download a file from `url` to `dest_path` on a remote host via SSH + curl.
///
/// Streams progress updates through `progress_tx`. Uses a one-shot SSH
/// connection with no ControlMaster (avoids mux session issues).
///
/// # Arguments
/// * `host` — Remote host address
/// * `user` — SSH user
/// * `url` — URL to download from
/// * `dest_path` — Full destination path on the remote host
/// * `progress_tx` — Channel for progress updates
pub async fn download_remote(
    host: &str,
    user: &str,
    url: &str,
    dest_path: &str,
    progress_tx: mpsc::Sender<DownloadProgress>,
) -> Result<()> {
    let cmd = format!("curl --progress-bar -o '{dest_path}' '{url}'");

    let mut child = tokio::process::Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=10",
            "-o", "StrictHostKeyChecking=accept-new",
            "-o", "ControlMaster=no",
            "-o", "ControlPath=none",
            &format!("{user}@{host}"),
            &cmd,
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| eyre!("Failed to spawn SSH: {e}"))?;

    // Read stderr for curl progress output
    if let Some(stderr) = child.stderr.take() {
        let mut reader = tokio::io::BufReader::new(stderr);
        let mut buf = [0u8; 256];

        loop {
            match reader.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    let chunk = String::from_utf8_lossy(&buf[..n]);
                    if let Some(pct) = parse_curl_percent(&chunk) {
                        let _ = progress_tx.send(DownloadProgress { percent: pct }).await;
                    }
                }
                Err(_) => break,
            }
        }
    }

    let status = child
        .wait()
        .await
        .map_err(|e| eyre!("SSH process error: {e}"))?;
    if !status.success() {
        return Err(eyre!(
            "Download failed (exit code {})",
            status.code().unwrap_or(-1)
        ));
    }

    let _ = progress_tx
        .send(DownloadProgress { percent: 100.0 })
        .await;

    Ok(())
}

/// Parse the last percentage value from a curl `--progress-bar` output chunk.
/// Looks for patterns like "12.5%" or "100.0%" in \r-separated output.
fn parse_curl_percent(chunk: &str) -> Option<f32> {
    chunk
        .split('\r')
        .filter_map(|segment| {
            let trimmed = segment.trim();
            if let Some(pos) = trimmed.rfind('%') {
                let before = &trimmed[..pos];
                let num_start = before
                    .rfind(|c: char| !c.is_ascii_digit() && c != '.')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                before[num_start..].parse::<f32>().ok()
            } else {
                None
            }
        })
        .last()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_curl_percent_simple() {
        assert_eq!(parse_curl_percent("  12.5%"), Some(12.5));
        assert_eq!(parse_curl_percent(" 100.0%"), Some(100.0));
        assert_eq!(parse_curl_percent("   0.0%"), Some(0.0));
    }

    #[test]
    fn test_parse_curl_percent_with_bar() {
        let chunk = "###########                                               21.9%";
        assert_eq!(parse_curl_percent(chunk), Some(21.9));
    }

    #[test]
    fn test_parse_curl_percent_multiple_cr() {
        let chunk = "## 3.1%\r### 4.7%\r#### 6.2%";
        assert_eq!(parse_curl_percent(chunk), Some(6.2));
    }

    #[test]
    fn test_parse_curl_percent_no_percent() {
        assert_eq!(parse_curl_percent("#=#=#"), None);
        assert_eq!(parse_curl_percent(""), None);
    }
}
