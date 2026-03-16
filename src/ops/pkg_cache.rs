//! Local caching proxies for Helios builds.
//!
//! Two containers:
//! - **nginx** (port 8888): Reverse proxy for pkg.oxide.computer (IPS packages).
//!   Helios `pkg` client talks HTTP to this, nginx fetches via HTTPS upstream.
//! - **squid** (port 3128): Forward proxy with SSL-bump for all other HTTPS
//!   downloads (xtask downloads, rustup, etc.). Helios tools use `https_proxy`
//!   env var to route through this.
//!
//! Both persist cached data in Docker named volumes across VM rebuilds.

use color_eyre::{eyre::eyre, Result};
use std::path::PathBuf;

const NGINX_CONTAINER: &str = "whoah-pkg-cache";
const NGINX_PORT: u16 = 8888;
const NGINX_IMAGE: &str = "nginx:alpine";

const SQUID_CONTAINER: &str = "whoah-https-cache";
const SQUID_PORT: u16 = 3128;
const SQUID_IMAGE: &str = "whoah-squid-ssl";

/// Result of ensuring all cache containers are running.
pub struct CacheInfo {
    /// The URL to use as the IPS publisher origin on Helios hosts.
    pub publisher_url: String,
    /// The HTTPS proxy URL for general downloads.
    pub https_proxy_url: String,
    /// The LAN IP the caches are reachable on.
    pub lan_ip: String,
    /// Whether the nginx container was already running.
    pub nginx_was_running: bool,
    /// Whether the squid container was already running.
    pub squid_was_running: bool,
}

/// Ensure both cache containers are running and reachable.
pub async fn ensure_caches() -> Result<CacheInfo> {
    let nginx_was_running = ensure_container_running(NGINX_CONTAINER).await;
    if !nginx_was_running {
        start_nginx().await?;
    }

    let squid_was_running = ensure_container_running(SQUID_CONTAINER).await;
    if !squid_was_running {
        start_squid().await?;
    }

    // Detect LAN IP after containers are running so we can test port reachability
    let lan_ip = detect_lan_ip().await?;

    let publisher_url = format!("http://{}:{}/helios/2/dev/", lan_ip, NGINX_PORT);
    let https_proxy_url = format!("http://{}:{}", lan_ip, SQUID_PORT);

    Ok(CacheInfo {
        publisher_url,
        https_proxy_url,
        lan_ip,
        nginx_was_running,
        squid_was_running,
    })
}

/// Legacy alias for code that only needs the pkg publisher URL.
pub async fn ensure_pkg_cache() -> Result<PkgCacheInfo> {
    let info = ensure_caches().await?;
    Ok(PkgCacheInfo {
        publisher_url: info.publisher_url,
        lan_ip: info.lan_ip,
        was_running: info.nginx_was_running,
    })
}

/// Legacy struct for backward compat.
pub struct PkgCacheInfo {
    pub publisher_url: String,
    pub lan_ip: String,
    pub was_running: bool,
}

/// Verify the pkg cache is reachable from a Helios host.
pub async fn verify_pkg_cache(
    host: &dyn crate::ssh::RemoteHost,
    publisher_url: &str,
) -> Result<bool> {
    let cmd = format!(
        "curl -sf --connect-timeout 5 --max-time 10 {publisher_url}catalog/1/catalog.attrs >/dev/null 2>&1"
    );
    let output = host.execute(&cmd).await?;
    Ok(output.exit_code == 0)
}

/// Verify the HTTPS proxy is reachable from a Helios host.
pub async fn verify_https_proxy(
    host: &dyn crate::ssh::RemoteHost,
    proxy_url: &str,
) -> Result<bool> {
    let cmd = format!(
        "curl -sf --connect-timeout 5 --max-time 10 --proxy {proxy_url} \
         -k https://pkg.oxide.computer/helios/2/dev/versions/0/ >/dev/null 2>&1"
    );
    let output = host.execute(&cmd).await?;
    Ok(output.exit_code == 0)
}

/// Set the pkg publisher on a Helios host to use the cache.
pub async fn set_publisher(
    host: &dyn crate::ssh::RemoteHost,
    publisher_url: &str,
) -> Result<()> {
    let cmd = format!("pfexec pkg set-publisher -O {publisher_url} helios-dev");
    let output = host.execute(&cmd).await?;
    if output.exit_code != 0 {
        return Err(eyre!(
            "Failed to set publisher: {}",
            output.stderr.trim()
        ));
    }
    Ok(())
}

/// Install the squid CA certificate on a Helios host so it trusts the HTTPS proxy.
/// Returns the path to the installed cert.
pub async fn install_ca_cert(
    host: &dyn crate::ssh::RemoteHost,
    _lan_ip: &str,
) -> Result<String> {
    let ca_cert = get_ca_cert().await?;
    let remote_cert_path = "/etc/certs/CA/whoah-cache-ca.pem";

    host.execute("pfexec mkdir -p /etc/certs/CA").await?;

    // Write cert via bash heredoc — quotes around EOF prevent any expansion
    let write_cmd = format!(
        "pfexec bash -c 'cat > {} << \"EOF\"\n{}\nEOF'",
        remote_cert_path, ca_cert.trim()
    );
    let output = host.execute(&write_cmd).await?;
    if output.exit_code != 0 {
        return Err(eyre!("Failed to write CA cert: {}", output.stderr.trim()));
    }

    // Verify
    let verify = host.execute(&format!(
        "openssl x509 -in {} -noout -subject", remote_cert_path
    )).await?;
    if verify.exit_code != 0 {
        return Err(eyre!("CA cert verification failed: {}", verify.stderr.trim()));
    }

    Ok(remote_cert_path.to_string())
}

/// Configure HTTPS proxy environment on a Helios host.
/// Writes to /etc/profile.d/ so it persists across SSH sessions.
pub async fn configure_proxy_env(
    host: &dyn crate::ssh::RemoteHost,
    proxy_url: &str,
    ca_cert_path: &str,
) -> Result<()> {
    let script = format!(
        r#"export https_proxy="{proxy_url}"
export HTTPS_PROXY="{proxy_url}"
export SSL_CERT_FILE="{ca_cert_path}"
export REQUESTS_CA_BUNDLE="{ca_cert_path}"
# Don't proxy local traffic
export no_proxy="localhost,127.0.0.1"
export NO_PROXY="localhost,127.0.0.1"
"#
    );

    let cmd = format!(
        "pfexec mkdir -p /etc/profile.d && printf '{}' | pfexec tee /etc/profile.d/whoah-proxy.sh > /dev/null",
        script.replace('\'', "'\\''")
    );
    let output = host.execute(&cmd).await?;
    if output.exit_code != 0 {
        return Err(eyre!("Failed to write proxy env: {}", output.stderr.trim()));
    }

    Ok(())
}

/// Get the CA certificate from the squid container.
async fn get_ca_cert() -> Result<String> {
    let output = tokio::process::Command::new("docker")
        .args(["exec", SQUID_CONTAINER, "cat", "/etc/squid/ssl/ca-cert.pem"])
        .output()
        .await
        .map_err(|e| eyre!("Failed to read CA cert from container: {e}"))?;

    if !output.status.success() {
        return Err(eyre!(
            "Failed to read CA cert: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

// ── Container management ────────────────────────────────────────

async fn ensure_container_running(name: &str) -> bool {
    let status = tokio::process::Command::new("docker")
        .args(["inspect", "-f", "{{.State.Running}}", name])
        .output()
        .await;

    match status {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim() == "true"
        }
        _ => false,
    }
}

async fn start_nginx() -> Result<()> {
    let _ = tokio::process::Command::new("docker")
        .args(["rm", "-f", NGINX_CONTAINER])
        .output()
        .await;

    let config_content = include_str!("../../assets/pkg-cache-nginx.conf");
    let config_dir = std::env::temp_dir().join("whoah-pkg-cache");
    tokio::fs::create_dir_all(&config_dir).await?;
    let config_path = config_dir.join("default.conf");
    tokio::fs::write(&config_path, config_content).await?;

    let output = tokio::process::Command::new("docker")
        .args([
            "run", "-d",
            "--restart=unless-stopped",
            "--name", NGINX_CONTAINER,
            "-p", &format!("{}:80", NGINX_PORT),
            "-v", &format!("{}:/etc/nginx/conf.d/default.conf:ro", config_path.display()),
            "-v", "whoah-pkg-cache:/var/cache/nginx/pkg",
            "-v", "whoah-pkg-cache-logs:/var/log/nginx",
            NGINX_IMAGE,
        ])
        .output()
        .await
        .map_err(|e| eyre!("Failed to start nginx: {e}"))?;

    if !output.status.success() {
        return Err(eyre!(
            "nginx start failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    Ok(())
}

async fn start_squid() -> Result<()> {
    let _ = tokio::process::Command::new("docker")
        .args(["rm", "-f", SQUID_CONTAINER])
        .output()
        .await;

    // Build the squid image if it doesn't exist
    build_squid_image().await?;

    let output = tokio::process::Command::new("docker")
        .args([
            "run", "-d",
            "--restart=unless-stopped",
            "--name", SQUID_CONTAINER,
            "-p", &format!("{}:3128", SQUID_PORT),
            "-v", "whoah-squid-ssl:/etc/squid/ssl",        // CA cert persists
            "-v", "whoah-squid-cache:/var/spool/squid",     // cache persists
            "-v", "whoah-squid-logs:/var/log/squid",        // logs persist
            SQUID_IMAGE,
        ])
        .output()
        .await
        .map_err(|e| eyre!("Failed to start squid: {e}"))?;

    if !output.status.success() {
        return Err(eyre!(
            "squid start failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    // Squid takes a moment to generate certs and initialize
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    Ok(())
}

async fn build_squid_image() -> Result<()> {
    // Check if image already exists
    let check = tokio::process::Command::new("docker")
        .args(["image", "inspect", SQUID_IMAGE])
        .output()
        .await;

    if let Ok(out) = check {
        if out.status.success() {
            return Ok(()); // Already built
        }
    }

    // Write build context to temp dir
    let build_dir = std::env::temp_dir().join("whoah-squid-build");
    tokio::fs::create_dir_all(&build_dir).await?;

    tokio::fs::write(
        build_dir.join("Dockerfile"),
        include_str!("../../assets/Dockerfile.squid-ssl"),
    ).await?;

    tokio::fs::write(
        build_dir.join("entrypoint.sh"),
        include_str!("../../assets/entrypoint.sh"),
    ).await?;

    tokio::fs::write(
        build_dir.join("squid.conf"),
        include_str!("../../assets/squid-ssl-bump.conf"),
    ).await?;

    // Build the image
    let output = tokio::process::Command::new("docker")
        .args([
            "build", "-t", SQUID_IMAGE,
            build_dir.to_str().unwrap(),
        ])
        .output()
        .await
        .map_err(|e| eyre!("Docker build failed: {e}"))?;

    if !output.status.success() {
        return Err(eyre!(
            "Docker build failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    Ok(())
}

// ── LAN IP detection ────────────────────────────────────────────

async fn detect_lan_ip() -> Result<String> {
    if is_wsl() {
        detect_lan_ip_wsl().await
    } else {
        detect_lan_ip_native().await
    }
}

fn is_wsl() -> bool {
    std::fs::read_to_string("/proc/version")
        .map(|v| v.to_lowercase().contains("microsoft"))
        .unwrap_or(false)
}

async fn detect_lan_ip_wsl() -> Result<String> {
    let output = tokio::process::Command::new("powershell.exe")
        .args([
            "-Command",
            r#"Get-NetIPAddress -AddressFamily IPv4 |
               Where-Object {
                   $_.InterfaceAlias -notlike '*Loopback*' -and
                   $_.InterfaceAlias -notlike '*vEthernet*' -and
                   $_.InterfaceAlias -notlike '*Bluetooth*' -and
                   $_.InterfaceAlias -notlike '*Local Area Connection*' -and
                   $_.PrefixOrigin -ne 'WellKnown' -and
                   ($_.IPAddress -like '192.168.*' -or $_.IPAddress -like '10.*')
               } |
               Select-Object -ExpandProperty IPAddress"#,
        ])
        .output()
        .await
        .map_err(|e| eyre!("PowerShell failed: {e}"))?;

    if !output.status.success() {
        return Err(eyre!(
            "PowerShell failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let ips: Vec<String> = stdout
        .lines()
        .map(|l| l.trim().trim_end().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    if ips.is_empty() {
        return Err(eyre!("No LAN IP found via PowerShell"));
    }

    // Test which IP actually has the Docker port reachable
    for ip in &ips {
        let check = tokio::process::Command::new("curl")
            .args([
                "-sf", "--connect-timeout", "2", "--max-time", "3",
                &format!("http://{}:{}/", ip, NGINX_PORT),
            ])
            .output()
            .await;

        if let Ok(out) = check {
            if out.status.success() || out.status.code() == Some(22) {
                tracing::info!("WSL LAN IP: {ip} is reachable on port {NGINX_PORT}");
                return Ok(ip.clone());
            }
        }
    }

    tracing::warn!("No IP responded on port {NGINX_PORT}, falling back to first 192.168.x.x");
    let ip = ips
        .iter()
        .find(|ip| ip.starts_with("192.168."))
        .or(ips.first())
        .ok_or_else(|| eyre!("No suitable LAN IP found"))?;

    Ok(ip.clone())
}

async fn detect_lan_ip_native() -> Result<String> {
    let output = tokio::process::Command::new("hostname")
        .args(["-I"])
        .output()
        .await
        .map_err(|e| eyre!("hostname -I failed: {e}"))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let ip = stdout
        .split_whitespace()
        .find(|ip| ip.starts_with("192.168.") || ip.starts_with("10."))
        .ok_or_else(|| eyre!("No LAN IP found"))?;

    Ok(ip.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_wsl_detection() {
        let _ = is_wsl();
    }
}
