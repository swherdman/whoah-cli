//! Host configuration discovery via one-shot SSH.
//!
//! Reads config from a running Helios host to populate deployment settings.
//! Uses one-shot SSH commands (no mux sessions) for compatibility with the
//! TUI's SSH approach. Designed for reuse in config drift detection.
//!
//! Replaces the older `import.rs` which used `RemoteHost` trait.
//! See docs/BACKLOG.md for deprecation plan.

use color_eyre::{eyre::eyre, Result};

use crate::config::types::*;
use crate::ssh::oneshot;

/// Configuration discovered from a running Helios host.
#[derive(Debug, Clone)]
pub struct DiscoveredConfig {
    pub network: NetworkConfig,
    pub overrides: OmicronOverrides,
    pub omicron_path: String,
}

/// Discover configuration from a running Helios host via one-shot SSH.
///
/// Reads config-rss.toml for network settings and config.toml + vdev files
/// for build overrides. Returns an error if config-rss.toml is not found
/// (Omicron not installed on host).
pub async fn discover_config(host: &str, user: &str) -> Result<DiscoveredConfig> {
    // Read config-rss.toml for network settings
    let rss_contents = ssh_command(
        host,
        user,
        "cat ~/omicron/smf/sled-agent/non-gimlet/config-rss.toml 2>/dev/null",
    )
    .await
    .map_err(|_| {
        eyre!("Could not read config-rss.toml — is ~/omicron present on {host}?")
    })?;

    let network = parse_network_from_rss(&rss_contents)?;

    // Read config.toml for vdev count
    let vdev_count = match ssh_command(
        host,
        user,
        "cat ~/omicron/smf/sled-agent/non-gimlet/config.toml 2>/dev/null",
    )
    .await
    {
        Ok(contents) => count_vdevs(&contents),
        Err(_) => 3,
    };

    // Read vdev size from disk
    let vdev_size_bytes = ssh_command(
        host,
        user,
        "ls -l /var/tmp/*.vdev 2>/dev/null | head -1 | awk '{print $5}'",
    )
    .await
    .ok()
    .and_then(|s| s.trim().parse::<u64>().ok());

    // Read COCKROACHDB_REDUNDANCY from source
    let cockroachdb_redundancy = ssh_command(
        host,
        user,
        "grep 'COCKROACHDB_REDUNDANCY' ~/omicron/common/src/policy.rs 2>/dev/null",
    )
    .await
    .ok()
    .and_then(|s| parse_rust_constant(&s));

    // Read storage buffer from source
    let storage_buffer_gib = ssh_command(
        host,
        user,
        "grep 'from_gibibytes_u32' ~/omicron/nexus/src/app/mod.rs 2>/dev/null",
    )
    .await
    .ok()
    .and_then(|s| parse_rust_constant(&s));

    Ok(DiscoveredConfig {
        network,
        overrides: OmicronOverrides {
            cockroachdb_redundancy,
            control_plane_storage_buffer_gib: storage_buffer_gib,
            vdev_count: Some(vdev_count),
            vdev_size_bytes,
        },
        omicron_path: "~/omicron".to_string(),
    })
}

/// Parse network config from config-rss.toml contents.
pub fn parse_network_from_rss(contents: &str) -> Result<NetworkConfig> {
    let table: toml::Value = toml::from_str(contents)
        .map_err(|e| eyre!("Failed to parse config-rss.toml: {e}"))?;

    let gateway = table
        .get("rack_network_config")
        .and_then(|r| {
            // Try routes first (more reliable)
            r.get("ports")
                .and_then(|p| p.as_array())
                .and_then(|a| a.first())
                .and_then(|p| p.get("routes"))
                .and_then(|r| r.as_array())
                .and_then(|a| a.first())
                .and_then(|r| r.get("nexthop"))
                .or_else(|| r.get("infra_ip_first"))
        })
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0.0")
        .to_string();

    let external_dns_ips = table
        .get("external_dns_ips")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let services_first = table
        .get("internal_services_ip_pool_ranges")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|r| r.get("first"))
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0.0")
        .to_string();

    let services_last = table
        .get("internal_services_ip_pool_ranges")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|r| r.get("last"))
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0.0")
        .to_string();

    let infra_ip = table
        .get("rack_network_config")
        .and_then(|r| r.get("infra_ip_first"))
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0.0")
        .to_string();

    // Instance pool — try allowed_source_ips.list, fall back to 0.0.0.0
    let instance_first = table
        .get("allowed_source_ips")
        .and_then(|v| v.get("list"))
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0.0")
        .to_string();

    let instance_last = table
        .get("allowed_source_ips")
        .and_then(|v| v.get("list"))
        .and_then(|v| v.as_array())
        .and_then(|a| a.last())
        .and_then(|v| v.as_str())
        .unwrap_or("0.0.0.0")
        .to_string();

    // Optional fields that are available in config-rss.toml
    let external_dns_zone_name = table
        .get("external_dns_zone_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let rack_subnet = table
        .get("rack_network_config")
        .and_then(|r| r.get("rack_subnet"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let ntp_servers = table
        .get("ntp_servers")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        });

    let dns_servers = table
        .get("dns_servers")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        });

    Ok(NetworkConfig {
        gateway,
        external_dns_ips,
        internal_services_range: IpRange {
            first: services_first,
            last: services_last,
        },
        infra_ip,
        instance_pool_range: IpRange {
            first: instance_first,
            last: instance_last,
        },
        ntp_servers,
        dns_servers,
        external_dns_zone_name,
        rack_subnet,
        uplink_port_speed: None,
        allowed_source_ips: None,
    })
}

/// Count vdev entries in config.toml.
pub fn count_vdevs(contents: &str) -> u32 {
    contents
        .lines()
        .filter(|l| l.contains(".vdev"))
        .count() as u32
}

/// Parse a u32 constant from a Rust source grep result.
pub fn parse_rust_constant(grep_output: &str) -> Option<u32> {
    let line = grep_output.trim();
    if line.is_empty() {
        return None;
    }
    if let Some(idx) = line.rfind('=') {
        let after = line[idx + 1..].trim().trim_end_matches(';').trim();
        if let Ok(n) = after.parse::<u32>() {
            return Some(n);
        }
    }
    if let Some(start) = line.rfind('(') {
        if let Some(end) = line.rfind(')') {
            if start < end {
                let inner = line[start + 1..end].trim();
                if let Ok(n) = inner.parse::<u32>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// Run a command on a remote host via one-shot SSH.
async fn ssh_command(host: &str, user: &str, cmd: &str) -> Result<String> {
    let output = oneshot::one_shot(host, user, cmd, 30).await?;
    if output.exit_code != 0 {
        return Err(eyre!("Command failed: {}", output.stderr.trim()));
    }
    Ok(output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_network_from_rss() {
        let toml = r#"
external_dns_ips = ["192.168.2.40", "192.168.2.41"]
external_dns_zone_name = "oxide.test"

[[internal_services_ip_pool_ranges]]
first = "192.168.2.40"
last = "192.168.2.49"

[rack_network_config]
infra_ip_first = "192.168.2.50"
infra_ip_last = "192.168.2.50"
rack_subnet = "fd00:1122:3344:0100::/56"

[[rack_network_config.ports]]
routes = [{nexthop = "192.168.2.1", destination = "0.0.0.0/0"}]
addresses = [{address = "192.168.2.30/24"}]
"#;
        let net = parse_network_from_rss(toml).unwrap();
        assert_eq!(net.gateway, "192.168.2.1");
        assert_eq!(net.external_dns_ips, vec!["192.168.2.40", "192.168.2.41"]);
        assert_eq!(net.internal_services_range.first, "192.168.2.40");
        assert_eq!(net.internal_services_range.last, "192.168.2.49");
        assert_eq!(net.infra_ip, "192.168.2.50");
        assert_eq!(net.external_dns_zone_name.as_deref(), Some("oxide.test"));
        assert_eq!(
            net.rack_subnet.as_deref(),
            Some("fd00:1122:3344:0100::/56")
        );
    }

    #[test]
    fn test_parse_network_minimal() {
        let toml = r#"
[rack_network_config]
infra_ip_first = "10.0.0.1"
"#;
        let net = parse_network_from_rss(toml).unwrap();
        assert_eq!(net.infra_ip, "10.0.0.1");
        // Defaults for missing fields
        assert!(net.external_dns_ips.is_empty());
        assert_eq!(net.internal_services_range.first, "0.0.0.0");
    }

    #[test]
    fn test_count_vdevs() {
        let config = r#"
[data]
vdevs = [
    "/var/tmp/u2_0.vdev",
    "/var/tmp/u2_1.vdev",
    "/var/tmp/u2_2.vdev",
]
"#;
        assert_eq!(count_vdevs(config), 3);
    }

    #[test]
    fn test_count_vdevs_empty() {
        assert_eq!(count_vdevs(""), 0);
    }

    #[test]
    fn test_parse_rust_constant_assignment() {
        assert_eq!(
            parse_rust_constant("pub const COCKROACHDB_REDUNDANCY: usize = 3;"),
            Some(3)
        );
    }

    #[test]
    fn test_parse_rust_constant_function_call() {
        assert_eq!(
            parse_rust_constant("    ByteCount::from_gibibytes_u32(5);"),
            Some(5)
        );
    }

    #[test]
    fn test_parse_rust_constant_empty() {
        assert_eq!(parse_rust_constant(""), None);
    }
}
