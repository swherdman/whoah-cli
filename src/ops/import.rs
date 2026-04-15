use color_eyre::{Result, eyre::eyre};

use crate::config::types::*;
use crate::ssh::RemoteHost;

/// Configuration discovered from a running Helios host.
#[derive(Debug, Clone)]
pub struct DiscoveredConfig {
    pub network: NetworkConfig,
    pub vdev_count: u32,
    pub vdev_size_bytes: Option<u64>,
    pub cockroachdb_redundancy: Option<u32>,
    pub storage_buffer_gib: Option<u32>,
    pub omicron_path: String,
}

/// Discover configuration from a running Helios host via SSH.
pub async fn discover_config(host: &dyn RemoteHost) -> Result<DiscoveredConfig> {
    // Read config-rss.toml for network settings
    let rss_output = host
        .execute("cat ~/omicron/smf/sled-agent/non-gimlet/config-rss.toml 2>/dev/null")
        .await?;
    if rss_output.exit_code != 0 {
        return Err(eyre!(
            "Could not read config-rss.toml — is ~/omicron present on {}?",
            host.hostname()
        ));
    }
    let network = parse_network_from_rss(&rss_output.stdout)?;

    // Read config.toml for vdev paths
    let config_output = host
        .execute("cat ~/omicron/smf/sled-agent/non-gimlet/config.toml 2>/dev/null")
        .await?;
    let vdev_count = if config_output.exit_code == 0 {
        count_vdevs(&config_output.stdout)
    } else {
        3 // default
    };

    // Read vdev sizes from disk
    let vdev_output = host
        .execute("ls -l /var/tmp/*.vdev 2>/dev/null | head -1 | awk '{print $5}'")
        .await?;
    let vdev_size_bytes = vdev_output.stdout.trim().parse::<u64>().ok();

    // Read COCKROACHDB_REDUNDANCY from source
    let crdb_output = host
        .execute("grep 'COCKROACHDB_REDUNDANCY' ~/omicron/common/src/policy.rs 2>/dev/null")
        .await?;
    let cockroachdb_redundancy = parse_rust_constant(&crdb_output.stdout);

    // Read CONTROL_PLANE_STORAGE_BUFFER from source
    let buffer_output = host
        .execute("grep 'from_gibibytes_u32' ~/omicron/nexus/src/app/mod.rs 2>/dev/null")
        .await?;
    let storage_buffer_gib = parse_rust_constant(&buffer_output.stdout);

    Ok(DiscoveredConfig {
        network,
        vdev_count,
        vdev_size_bytes,
        cockroachdb_redundancy,
        storage_buffer_gib,
        omicron_path: "~/omicron".to_string(),
    })
}

/// Parse network config from config-rss.toml contents.
fn parse_network_from_rss(contents: &str) -> Result<NetworkConfig> {
    // config-rss.toml is TOML — parse it
    let table: toml::Value =
        toml::from_str(contents).map_err(|e| eyre!("Failed to parse config-rss.toml: {e}"))?;

    let gateway = table
        .get("rack_network_config")
        .and_then(|r| r.get("infra_ip_first"))
        .or_else(|| {
            // Try to find gateway from routes
            table
                .get("rack_network_config")
                .and_then(|r| r.get("routes"))
                .and_then(|r| r.as_array())
                .and_then(|a| a.first())
                .and_then(|r| r.get("nexthop"))
        })
        .and_then(|v| v.as_str())
        .unwrap_or("192.168.1.1")
        .to_string();

    let external_dns_ips = table
        .get("external_dns_ips")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_else(|| vec!["192.168.1.20".to_string()]);

    let services_first = table
        .get("internal_services_ip_pool_ranges")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|r| r.get("first"))
        .and_then(|v| v.as_str())
        .unwrap_or("192.168.1.20")
        .to_string();

    let services_last = table
        .get("internal_services_ip_pool_ranges")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|r| r.get("last"))
        .and_then(|v| v.as_str())
        .unwrap_or("192.168.1.29")
        .to_string();

    let infra_first = table
        .get("rack_network_config")
        .and_then(|r| r.get("infra_ip_first"))
        .and_then(|v| v.as_str())
        .unwrap_or("192.168.1.30")
        .to_string();

    // Look for instance IP pool in allowed_source_ips or similar
    let instance_first = table
        .get("allowed_source_ips")
        .and_then(|v| v.get("list"))
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_str())
        .unwrap_or("192.168.1.31")
        .to_string();

    let instance_last = table
        .get("allowed_source_ips")
        .and_then(|v| v.get("list"))
        .and_then(|v| v.as_array())
        .and_then(|a| a.last())
        .and_then(|v| v.as_str())
        .unwrap_or("192.168.1.40")
        .to_string();

    Ok(NetworkConfig {
        gateway,
        external_dns_ips,
        internal_services_range: IpRange {
            first: services_first,
            last: services_last,
        },
        infra_ip: infra_first,
        instance_pool_range: IpRange {
            first: instance_first,
            last: instance_last,
        },
        ntp_servers: None,
        dns_servers: None,
        external_dns_zone_name: None,
        rack_subnet: None,
        uplink_port_speed: None,
        allowed_source_ips: None,
    })
}

/// Count vdev entries in config.toml.
fn count_vdevs(contents: &str) -> u32 {
    contents.lines().filter(|l| l.contains(".vdev")).count() as u32
}

/// Parse a u32 constant from a Rust source grep result.
/// Matches patterns like `= 3;` or `_u32(5)`.
fn parse_rust_constant(grep_output: &str) -> Option<u32> {
    let line = grep_output.trim();
    if line.is_empty() {
        return None;
    }
    // Try matching `= NUMBER;`
    if let Some(idx) = line.rfind('=') {
        let after = line[idx + 1..].trim().trim_end_matches(';').trim();
        if let Ok(n) = after.parse::<u32>() {
            return Some(n);
        }
    }
    // Try matching `_u32(NUMBER)` or `_gibibytes_u32(NUMBER)`
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
