//! Remote file download via SSH + curl with streaming progress.
//!
//! Opens an ephemeral russh connection, runs `curl --progress-bar` on the
//! remote host, parses stderr for progress updates, and streams them back
//! via an mpsc channel.

use std::sync::Arc;
use std::time::Duration;

use color_eyre::{eyre::eyre, Result};
use russh::ChannelMsg;
use tokio::sync::mpsc;

use super::auth::authenticate;
use super::handler::SshClientHandler;

/// Download progress update.
#[derive(Debug, Clone)]
pub struct DownloadProgress {
    /// Percentage complete (0.0 - 100.0).
    pub percent: f32,
}

/// Download a file from `url` to `dest_path` on a remote host via SSH + curl.
///
/// Streams progress updates through `progress_tx`. Uses an ephemeral russh
/// connection (no persistent session needed).
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

    let cmd = format!("curl --progress-bar -o '{dest_path}' '{url}'");
    channel
        .exec(true, cmd.as_bytes())
        .await
        .map_err(|e| eyre!("Failed to exec curl on {host}: {e}"))?;

    let mut exit_code: Option<u32> = None;

    loop {
        match channel.wait().await {
            Some(ChannelMsg::ExtendedData { data, ext }) if ext == 1 => {
                // curl sends progress to stderr
                let chunk = String::from_utf8_lossy(&data);
                if let Some(pct) = parse_curl_percent(&chunk) {
                    let _ = progress_tx.send(DownloadProgress { percent: pct }).await;
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

    let _ = handle
        .disconnect(russh::Disconnect::ByApplication, "", "en")
        .await;

    let code = exit_code.unwrap_or(1);
    if code != 0 {
        return Err(eyre!("Download failed (exit code {code})"));
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
