use std::collections::HashMap;

use color_eyre::Result;
use serde::Serialize;

use crate::config::types::derive_expected_zones;
use crate::config::DeploymentConfig;
use crate::parse::{disk, network, services, zones, zpool};
use crate::ssh::{CommandOutput, RemoteHost};

#[derive(Debug, Clone, Serialize)]
pub struct HostStatus {
    pub hostname: String,
    pub connection: ConnectionState,
    pub services: ServiceStatus,
    pub zones: ZoneStatus,
    pub disk: DiskStatus,
    pub network: NetworkStatus,
    pub reboot_detected: bool,
}

#[derive(Debug, Clone, Serialize)]
pub enum ConnectionState {
    Connected,
    Disconnected { error: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceStatus {
    pub sled_agent: Option<services::ServiceState>,
    pub baseline: Option<services::ServiceState>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ZoneStatus {
    /// Actual running count per service name
    pub service_counts: HashMap<String, u32>,
    /// Expected count per service name (from config)
    pub expected_services: HashMap<String, u32>,
    /// Running VM instance count
    pub instance_count: u32,
    pub zones: Vec<zones::ZoneInfo>,
    pub placement: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiskStatus {
    pub rpool: Option<zpool::ZpoolInfo>,
    pub oxp_pools: Vec<zpool::ZpoolInfo>,
    pub vdev_files: Vec<disk::VdevFileInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NetworkStatus {
    pub nexus_reachable: bool,
    pub dns_resolving: bool,
    pub dns_addresses: Vec<String>,
    pub simnets_exist: bool,
}

/// Gather complete status from a host.
/// Runs status commands concurrently and parses their output.
pub async fn gather_status(
    host: &dyn RemoteHost,
    config: &DeploymentConfig,
) -> Result<HostStatus> {
    let dns_ip = config
        .deployment
        .network
        .external_dns_ips
        .first()
        .cloned()
        .unwrap_or_default();

    // Phase 1: resolve DNS first, then use result to find Nexus
    let dns_cmd = format!("dig recovery.sys.oxide.test @{dns_ip} +short +time=3 +tries=1 2>/dev/null");

    // Run DNS + all non-Nexus commands concurrently
    let (zpool_result, zone_result, svcs_result, vdev_result, dns_result, simnet_result) = tokio::join!(
        host.execute("zpool list -Hp"),
        host.execute("zoneadm list -cp"),
        host.execute("svcs -H -o state,fmri sled-agent omicron/baseline 2>/dev/null"),
        host.execute("ls -s /var/tmp/*.vdev 2>/dev/null"),
        host.execute(&dns_cmd),
        host.execute("dladm show-simnet 2>/dev/null"),
    );

    // Parse DNS result to discover Nexus IP
    let dns_output = dns_result.unwrap_or_else(|_| CommandOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 1,
    });
    let dns_check = network::parse_dns_check(&dns_output.stdout);

    // Ping Nexus using the DNS-resolved address
    let ping_result = if let Some(nexus_ip) = dns_check.addresses.first() {
        let ping_cmd = format!(
            "curl -sf --connect-timeout 3 --max-time 5 http://{nexus_ip}/v1/ping 2>/dev/null"
        );
        host.execute(&ping_cmd).await
    } else {
        // DNS didn't resolve — Nexus is unreachable
        Ok(CommandOutput {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 1,
        })
    };

    // Parse zpool
    let zpool_output = zpool_result.unwrap_or_else(|_| CommandOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 1,
    });
    let rpool = zpool::parse_rpool(&zpool_output.stdout).unwrap_or(None);
    let oxp_pools = zpool::parse_oxp_pools(&zpool_output.stdout).unwrap_or_default();

    // Parse zones
    let zone_output = zone_result.unwrap_or_else(|_| CommandOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 1,
    });
    let zone_list = zones::parse_zoneadm_list(&zone_output.stdout).unwrap_or_default();
    let running_zones: Vec<_> = zone_list
        .iter()
        .filter(|z| z.status == zones::ZoneStatus::Running)
        .collect();

    // Build per-zone-type counts (services + infra together, instances separate)
    let mut service_counts: HashMap<String, u32> = HashMap::new();
    let mut instance_count: u32 = 0;
    for zone in &running_zones {
        match zone.kind {
            zones::ZoneKind::Service | zones::ZoneKind::Infrastructure => {
                *service_counts.entry(zone.service_name.clone()).or_insert(0) += 1;
            }
            zones::ZoneKind::Instance => instance_count += 1,
        }
    }

    let placement = zones::derive_zone_placement(&zone_list);

    // Parse services
    let svcs_output = svcs_result.unwrap_or_else(|_| CommandOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 1,
    });
    let svc_list = services::parse_svcs(&svcs_output.stdout).unwrap_or_default();
    let sled_agent = svc_list
        .iter()
        .find(|s| s.fmri.contains("sled-agent"))
        .map(|s| s.state.clone());
    let baseline = svc_list
        .iter()
        .find(|s| s.fmri.contains("baseline"))
        .map(|s| s.state.clone());

    // Parse vdev files
    let vdev_output = vdev_result.unwrap_or_else(|_| CommandOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 1,
    });
    let vdev_files = disk::parse_vdev_files(&vdev_output.stdout).unwrap_or_default();

    // Parse network checks
    let ping_output = ping_result.unwrap_or_else(|_| CommandOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 1,
    });
    let nexus_reachable = network::parse_nexus_ping(ping_output.exit_code);

    // dns_check already parsed above (used to resolve Nexus IP)

    let simnet_output = simnet_result.unwrap_or_else(|_| CommandOutput {
        stdout: String::new(),
        stderr: String::new(),
        exit_code: 1,
    });
    let simnets_exist = network::parse_simnet_check(simnet_output.exit_code, &simnet_output.stdout);

    let network_status = NetworkStatus {
        nexus_reachable,
        dns_resolving: dns_check.resolved,
        dns_addresses: dns_check.addresses,
        simnets_exist,
    };

    let total_services: u32 = service_counts.values().sum();
    let reboot_detected = is_post_reboot_from_parts(
        simnets_exist,
        total_services,
        &baseline,
    );

    Ok(HostStatus {
        hostname: host.hostname().to_string(),
        connection: ConnectionState::Connected,
        services: ServiceStatus {
            sled_agent,
            baseline,
        },
        zones: ZoneStatus {
            service_counts,
            expected_services: derive_expected_zones(&config.build.omicron.overrides),
            instance_count,
            zones: zone_list,
            placement,
        },
        disk: DiskStatus {
            rpool,
            oxp_pools,
            vdev_files,
        },
        network: network_status,
        reboot_detected,
    })
}

/// Detect if the host is in post-reboot state.
pub fn is_post_reboot(status: &HostStatus) -> bool {
    let total_services: u32 = status.zones.service_counts.values().sum();
    is_post_reboot_from_parts(
        status.network.simnets_exist,
        total_services,
        &status.services.baseline,
    )
}

fn is_post_reboot_from_parts(
    simnets_exist: bool,
    running_zones: u32,
    baseline: &Option<services::ServiceState>,
) -> bool {
    // No simnets = definitely rebooted (they're temporary)
    if !simnets_exist {
        return true;
    }
    // No running zones (other than global) = rebooted
    if running_zones == 0 {
        return true;
    }
    // Baseline offline or in maintenance after reboot
    if let Some(state) = baseline {
        if *state == services::ServiceState::Offline
            || *state == services::ServiceState::Maintenance
        {
            // Only flag if there are also no zones
            return running_zones == 0;
        }
    }
    false
}

/// Format a status report for headless output.
pub fn format_status(status: &HostStatus, deployment_name: &str) -> String {
    let mut out = String::new();

    // Header
    out.push_str(&format!(
        "{} / {} ({})\n",
        deployment_name,
        status.hostname,
        match &status.connection {
            ConnectionState::Connected => "connected",
            ConnectionState::Disconnected { .. } => "disconnected",
        }
    ));

    // Reboot warning
    if status.reboot_detected {
        out.push_str("  *** REBOOT DETECTED — run 'whoah recover' ***\n");
    }

    // Services
    let sled = status
        .services
        .sled_agent
        .as_ref()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "not found".to_string());
    let base = status
        .services
        .baseline
        .as_ref()
        .map(|s| s.to_string())
        .unwrap_or_else(|| "not found".to_string());
    out.push_str(&format!("  Services:  sled-agent: {sled}, baseline: {base}\n"));

    // Per-service zone counts
    // Collect all service names from both expected and actual
    let mut all_services: Vec<String> = status.zones.expected_services.keys().cloned().collect();
    for svc in status.zones.service_counts.keys() {
        if !all_services.contains(svc) {
            all_services.push(svc.clone());
        }
    }
    all_services.sort();

    for svc in &all_services {
        let actual = status.zones.service_counts.get(svc).copied().unwrap_or(0);
        let expected = status.zones.expected_services.get(svc).copied();
        let marker = match expected {
            Some(exp) if actual < exp => " (!)",
            _ => "",
        };
        let expected_str = match expected {
            Some(exp) => format!("/{exp}"),
            None => String::new(),
        };
        out.push_str(&format!(
            "  {svc:<18} {actual}{expected_str}{marker}\n"
        ));
    }

    // Instances
    if status.zones.instance_count > 0 {
        let instance_names: Vec<String> = status
            .zones
            .zones
            .iter()
            .filter(|z| z.kind == zones::ZoneKind::Instance && z.status == zones::ZoneStatus::Running)
            .map(|z| z.service_name.clone())
            .collect();
        out.push_str(&format!(
            "  Instances:         {} ({})\n",
            status.zones.instance_count,
            instance_names.join(", ")
        ));
    }

    // Zone placement
    if !status.zones.placement.is_empty() {
        let mut pools: Vec<_> = status.zones.placement.iter().collect();
        pools.sort_by_key(|(k, _)| (*k).clone());
        for (pool, zone_names) in pools {
            // Shorten pool name for display
            let short_pool = pool
                .strip_prefix("oxp_")
                .map(|s| {
                    if s.len() > 8 {
                        format!("oxp_{}...", &s[..8])
                    } else {
                        format!("oxp_{s}")
                    }
                })
                .unwrap_or_else(|| pool.clone());
            out.push_str(&format!("             {short_pool}: {}\n", zone_names.join(", ")));
        }
    }

    // Disk
    if let Some(rpool) = &status.disk.rpool {
        let free_gib = rpool.free_bytes as f64 / 1_073_741_824.0;
        out.push_str(&format!(
            "  rpool:     {}% ({:.0} GiB free)\n",
            rpool.capacity_pct, free_gib
        ));
    }
    if !status.disk.oxp_pools.is_empty() {
        let pool_strs: Vec<String> = status
            .disk
            .oxp_pools
            .iter()
            .map(|p| {
                let short = p.name.strip_prefix("oxp_").map(|s| {
                    if s.len() > 8 { format!("oxp_{}...", &s[..8]) } else { format!("oxp_{s}") }
                }).unwrap_or_else(|| p.name.clone());
                format!("{short}: {}%", p.capacity_pct)
            })
            .collect();
        out.push_str(&format!("  oxp pools: {}\n", pool_strs.join(", ")));
    }
    if !status.disk.vdev_files.is_empty() {
        let vdev_strs: Vec<String> = status
            .disk
            .vdev_files
            .iter()
            .map(|v| {
                let name = v
                    .path
                    .rsplit('/')
                    .next()
                    .unwrap_or(&v.path);
                let gib = v.size_bytes as f64 / 1_073_741_824.0;
                format!("{name}: {gib:.1} GiB")
            })
            .collect();
        out.push_str(&format!("  vdevs:     {}\n", vdev_strs.join(", ")));
    }

    // Network
    let nexus = if status.network.nexus_reachable {
        "reachable"
    } else {
        "unreachable"
    };
    let dns = if status.network.dns_resolving {
        "resolving"
    } else {
        "not resolving"
    };
    out.push_str(&format!("  Nexus:     {nexus}\n"));
    out.push_str(&format!("  DNS:       {dns}\n"));

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssh::mock::MockHost;
    use crate::config::types::*;
    use std::collections::BTreeMap;

    fn sample_config() -> DeploymentConfig {
        DeploymentConfig {
            deployment: DeploymentToml {
                deployment: DeploymentMeta {
                    name: "test-lab".to_string(),
                    description: None,
                },
                hosts: {
                    let mut h = BTreeMap::new();
                    h.insert("helios01".to_string(), HostConfig {
                        address: "192.168.2.209".to_string(),
                        ssh_user: "testuser".to_string(),
                        role: HostRole::Combined,
                        host_type: None,
                    });
                    h
                },
                network: NetworkConfig {
                    gateway: "192.168.2.1".to_string(),
                    external_dns_ips: vec!["192.168.2.70".to_string()],
                    internal_services_range: IpRange {
                        first: "192.168.2.70".to_string(),
                        last: "192.168.2.79".to_string(),
                    },
                    infra_ip: "192.168.2.80".to_string(),
                    instance_pool_range: IpRange {
                        first: "192.168.2.81".to_string(),
                        last: "192.168.2.90".to_string(),
                    },
                    ntp_servers: None,
                    dns_servers: None,
                    external_dns_zone_name: None,
                    rack_subnet: None,
                    uplink_port_speed: None,
                    allowed_source_ips: None,
                },
                nexus: NexusConfig::default(),
                hypervisor: None,
            },
            build: BuildToml::default(),
            monitoring: MonitoringToml::default(),
        }
    }

    fn setup_healthy_mock() -> MockHost {
        let mut mock = MockHost::new("192.168.2.209");
        mock.add_success(
            "zpool list -Hp",
            "rpool\t267544698880\t82530148352\t185014550528\t-\t-\t25\t30\t1.00\tONLINE\t-\n\
             oxp_aaa\t42949672960\t16267415552\t26682257408\t-\t-\t10\t38\t1.00\tONLINE\t-\n",
        );
        mock.add_success(
            "zoneadm list -cp",
            "0:global:running:/::\tipkg:shared\n\
             1:oxz_cockroachdb_abc123:running:/pool/ext/oxp_aaa/crypt/zone/oxz_cockroachdb_abc123:abc:omicron1:excl\n\
             2:oxz_nexus_def456:running:/pool/ext/oxp_aaa/crypt/zone/oxz_nexus_def456:def:omicron1:excl\n",
        );
        mock.add_success(
            "svcs -H",
            "online         svc:/system/sled-agent:default\n\
             online         svc:/system/omicron/baseline:default\n",
        );
        mock.add_success("ls -s /var/tmp", " 24313856 /var/tmp/u2_0.vdev\n");
        mock.add_success("curl", "");
        mock.add_success("dig", "192.168.2.72\n");
        mock.add_success("dladm show-simnet", "net0\tnet1\n");
        mock
    }

    #[tokio::test]
    async fn test_gather_status_healthy() {
        let mock = setup_healthy_mock();
        let config = sample_config();
        let status = gather_status(&mock, &config).await.unwrap();

        assert!(!status.reboot_detected);
        let total: u32 = status.zones.service_counts.values().sum();
        assert_eq!(total, 2);
        assert!(status.network.nexus_reachable);
        assert!(status.network.dns_resolving);
        assert!(status.network.simnets_exist);
        assert!(status.disk.rpool.is_some());
        assert_eq!(status.disk.rpool.as_ref().unwrap().capacity_pct, 30);
    }

    #[tokio::test]
    async fn test_gather_status_post_reboot() {
        let mut mock = MockHost::new("192.168.2.209");
        mock.add_success("zpool list -Hp", "rpool\t267544698880\t82530148352\t185014550528\t-\t-\t25\t30\t1.00\tONLINE\t-\n");
        mock.add_success("zoneadm list -cp", "0:global:running:/::\tipkg:shared\n");
        mock.add_success("svcs -H", "offline        svc:/system/sled-agent:default\n");
        mock.add_failure("ls -s /var/tmp", "No such file", 1);
        mock.add_failure("curl", "", 22);
        mock.add_success("dig", "");
        mock.add_failure("dladm show-simnet", "", 1);

        let config = sample_config();
        let status = gather_status(&mock, &config).await.unwrap();

        assert!(status.reboot_detected);
        let total: u32 = status.zones.service_counts.values().sum();
        assert_eq!(total, 0);
        assert!(!status.network.nexus_reachable);
        assert!(!status.network.simnets_exist);
    }

    #[test]
    fn test_format_status() {
        let status = HostStatus {
            hostname: "192.168.2.209".to_string(),
            connection: ConnectionState::Connected,
            services: ServiceStatus {
                sled_agent: Some(services::ServiceState::Online),
                baseline: Some(services::ServiceState::Online),
            },
            zones: ZoneStatus {
                service_counts: HashMap::from([
                    ("nexus".to_string(), 3),
                    ("cockroachdb".to_string(), 3),
                ]),
                expected_services: HashMap::from([
                    ("nexus".to_string(), 3),
                    ("cockroachdb".to_string(), 3),
                ]),
                instance_count: 0,
                zones: vec![],
                placement: HashMap::new(),
            },
            disk: DiskStatus {
                rpool: Some(zpool::ZpoolInfo {
                    name: "rpool".to_string(),
                    size_bytes: 267544698880,
                    allocated_bytes: 82530148352,
                    free_bytes: 185014550528,
                    fragmentation_pct: Some(25),
                    capacity_pct: 30,
                    health: "ONLINE".to_string(),
                }),
                oxp_pools: vec![],
                vdev_files: vec![],
            },
            network: NetworkStatus {
                nexus_reachable: true,
                dns_resolving: true,
                dns_addresses: vec!["192.168.2.72".to_string()],
                simnets_exist: true,
            },
            reboot_detected: false,
        };
        let output = format_status(&status, "test-lab");
        assert!(output.contains("test-lab"));
        assert!(output.contains("nexus"));
        assert!(output.contains("3/3"));
        assert!(output.contains("30%"));
        assert!(output.contains("reachable"));
    }
}
