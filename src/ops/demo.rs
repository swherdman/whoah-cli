//! Demo mode.
//!
//! Provides simulated data for the TUI so it can be demonstrated without
//! real infrastructure. The build pipeline sends the same `BuildEvent`
//! messages with proportional delays. The monitor dashboard gets a
//! realistic `HostStatus` snapshot. The TUI renders identically — it
//! doesn't know the difference.

use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::config::types::{DeploymentConfig, derive_expected_zones};
use crate::event::BuildEvent;
use crate::ops::pipeline::build_deploy_pipeline;
use crate::ops::status::{
    ConnectionState, DiskStatus, HostStatus, NetworkStatus, ServiceStatus, ZoneStatus,
};
use crate::parse::{disk, services, zones, zpool};

/// Realistic duration for each step, averaged from real build logs.
/// Used by the App to override the displayed duration in demo mode.
pub fn realistic_duration(id: &str) -> Duration {
    Duration::from_secs(match id {
        "prov-create" => 1,
        "prov-boot" => 2,
        "prov-install" => 44,
        "vm-network" => 35,
        "vm-netcat" => 1,
        "access-keys" => 5,
        "access-verify" => 1,
        "cache-start" => 90,
        "cache-configure" => 20,
        "os-update" => 150,
        "os-reboot" => 50,
        "os-cleanup" => 2,
        "os-packages" => 167,
        "os-rust" => 40,
        "os-swap" => 2,
        "repo-clone" => 24,
        "repo-configure" => 2,
        "build-prereqs-builder" => 768,
        "build-prereqs-runner" => 97,
        "build-fix-perms" => 2,
        "build-compile" => 2415,
        "build-package" => 300,
        "build-patch" => 11,
        "deploy-vhw" => 114,
        "deploy-install" => 404,
        "deploy-verify" => 458,
        "config-quotas" => 195,
        "config-ippool" => 1,
        _ => 5,
    })
}

/// Proportional weight for each step. Steps not listed default to 1.
/// Weights are derived from averaged real-world durations across multiple runs.
fn weight(id: &str) -> u32 {
    match id {
        "prov-create" => 1,
        "prov-boot" => 1,
        "prov-install" => 2,
        "vm-network" => 2,
        "vm-netcat" => 1,
        "access-keys" => 1,
        "access-verify" => 1,
        "cache-start" => 2,
        "cache-configure" => 1,
        "os-update" => 3,
        "os-reboot" => 2,
        "os-cleanup" => 1,
        "os-packages" => 3,
        "os-rust" => 2,
        "os-swap" => 1,
        "repo-clone" => 1,
        "repo-configure" => 1,
        "build-prereqs-builder" => 8,
        "build-prereqs-runner" => 2,
        "build-fix-perms" => 1,
        "build-compile" => 15,
        "build-package" => 4,
        "build-patch" => 1,
        "deploy-vhw" => 2,
        "deploy-install" => 5,
        "deploy-verify" => 5,
        "config-quotas" => 3,
        "config-ippool" => 1,
        _ => 1,
    }
}

/// Duration per weight unit. Total demo ≈ 73 × 350ms ≈ 25s.
const SCALE_MS: u64 = 350;

/// Run a simulated pipeline, sending `BuildEvent`s with proportional delays.
pub async fn run_demo(tx: mpsc::UnboundedSender<BuildEvent>) {
    let pipeline = build_deploy_pipeline();

    for phase in &pipeline.phases {
        for step in &phase.steps {
            let w = weight(step.id);
            let step_duration = Duration::from_millis(SCALE_MS * w as u64);

            // Start
            let _ = tx.send(BuildEvent::StepStarted(step.id.into()));

            // Simulate detail output — spread lines across the step duration
            let detail = format!("{}...", step.name);
            let line_delay = step_duration / 3;

            tokio::time::sleep(line_delay).await;
            let _ = tx.send(BuildEvent::StepDetail(step.id.into(), detail));

            tokio::time::sleep(line_delay).await;
            let _ = tx.send(BuildEvent::StepDetail(
                step.id.into(),
                format!("{} in progress", step.name),
            ));

            tokio::time::sleep(line_delay).await;

            // Complete
            let _ = tx.send(BuildEvent::StepCompleted(step.id.into()));
        }
    }
}

// ── Monitor demo data ──────────────────────────────────────────

const OXP_UUIDS: [&str; 3] = [
    "af5cfbf7-0f55-4c04-b695-dc5a42f0c06e",
    "b31e8a9c-72d1-4f8b-a3c5-9e6d47281fa3",
    "c8a24d15-e963-41ab-8f72-3b0c59d8e7a1",
];

/// Build a realistic `HostStatus` for the Monitor screen in demo mode.
pub fn demo_status(config: &DeploymentConfig) -> HostStatus {
    let expected = derive_expected_zones(&config.build.omicron.overrides);

    // Build zone list and placement from expected counts
    let mut zone_list: Vec<zones::ZoneInfo> = Vec::new();
    let mut placement: HashMap<String, Vec<String>> = HashMap::new();
    let mut service_counts: HashMap<String, u32> = HashMap::new();

    let pool_count = OXP_UUIDS.len();
    let mut zone_id = 1u32;
    let mut pool_cursor = 0usize; // global cursor for even distribution

    for (svc, &count) in &expected {
        service_counts.insert(svc.clone(), count);
        for _ in 0..count {
            let pool_uuid = OXP_UUIDS[pool_cursor % pool_count];
            pool_cursor += 1;
            let pool_name = format!("oxp_{pool_uuid}");
            let zone_name = format!("oxz_{svc}_{zone_id:08x}");

            let kind = match svc.as_str() {
                "switch" | "sidecar_softnpu" => zones::ZoneKind::Infrastructure,
                _ => zones::ZoneKind::Service,
            };

            zone_list.push(zones::ZoneInfo {
                id: Some(zone_id),
                name: zone_name.clone(),
                status: zones::ZoneStatus::Running,
                path: format!("/pool/ext/{pool_uuid}/crypt/zone/{zone_name}"),
                uuid: format!("{zone_id:08x}-0000-0000-0000-000000000000"),
                brand: "omicron1".into(),
                ip_type: "excl".into(),
                kind,
                service_name: svc.clone(),
            });

            placement.entry(pool_name).or_default().push(svc.clone());

            zone_id += 1;
        }
    }

    // Pools
    let rpool = Some(zpool::ZpoolInfo {
        name: "rpool".into(),
        size_bytes: 267_544_698_880,     // 249 GiB
        allocated_bytes: 82_530_148_352, // ~77 GiB
        free_bytes: 185_014_550_528,     // ~172 GiB
        fragmentation_pct: Some(25),
        capacity_pct: 30,
        health: "ONLINE".into(),
    });

    let oxp_pools: Vec<zpool::ZpoolInfo> = OXP_UUIDS
        .iter()
        .zip([28u8, 35, 42])
        .map(|(uuid, pct)| {
            let size: u64 = 42_949_672_960; // 40 GiB
            let alloc = size * pct as u64 / 100;
            zpool::ZpoolInfo {
                name: format!("oxp_{uuid}"),
                size_bytes: size,
                allocated_bytes: alloc,
                free_bytes: size - alloc,
                fragmentation_pct: Some(10),
                capacity_pct: pct,
                health: "ONLINE".into(),
            }
        })
        .collect();

    // Vdev files: 3 u2 + 2 m2
    // These are sparse files — ls -s reports actual allocated blocks, not logical size.
    // Realistic allocated sizes are much smaller than the logical vdev_size_bytes.
    let u2_sizes: [u64; 3] = [
        12_884_901_888, // 12.0 GiB
        14_262_534_144, // 13.3 GiB
        11_811_160_064, // 11.0 GiB
    ];
    let mut vdev_files = Vec::new();
    for (i, &size) in u2_sizes.iter().enumerate() {
        vdev_files.push(disk::VdevFileInfo {
            path: format!("/var/tmp/u2_{i}.vdev"),
            size_blocks: size / 512,
            size_bytes: size,
        });
    }
    let m2_sizes: [u64; 2] = [
        3_221_225_472, // 3.0 GiB
        3_758_096_384, // 3.5 GiB
    ];
    for (i, &size) in m2_sizes.iter().enumerate() {
        vdev_files.push(disk::VdevFileInfo {
            path: format!("/var/tmp/m2_{i}.vdev"),
            size_blocks: size / 512,
            size_bytes: size,
        });
    }

    let dns_ip = config
        .deployment
        .network
        .external_dns_ips
        .first()
        .cloned()
        .unwrap_or_else(|| "192.168.2.40".into());

    HostStatus {
        hostname: config
            .deployment
            .hosts
            .values()
            .next()
            .map(|h| h.address.clone())
            .unwrap_or_else(|| "192.168.2.100".into()),
        connection: ConnectionState::Connected,
        services: ServiceStatus {
            sled_agent: Some(services::ServiceState::Online),
            baseline: Some(services::ServiceState::Online),
        },
        zones: ZoneStatus {
            service_counts,
            expected_services: expected,
            instance_count: 0,
            zones: zone_list,
            placement,
        },
        disk: DiskStatus {
            rpool,
            oxp_pools,
            vdev_files,
        },
        network: NetworkStatus {
            nexus_reachable: true,
            dns_resolving: true,
            dns_addresses: vec![dns_ip],
            simnets_exist: true,
        },
        reboot_detected: false,
    }
}
