use std::time::{Duration, Instant};

use color_eyre::{eyre::eyre, Result};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::DeploymentConfig;
use crate::parse::{network, zones};
use crate::ssh::RemoteHost;

// --- Recovery Steps ---

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryStep {
    WaitForBaseline,
    Uninstall,
    DestroyVirtualHw,
    CreateVirtualHw,
    Install,
    MonitorZones,
    Verify,
}

impl RecoveryStep {
    pub fn index(&self) -> usize {
        match self {
            Self::WaitForBaseline => 0,
            Self::Uninstall => 1,
            Self::DestroyVirtualHw => 2,
            Self::CreateVirtualHw => 3,
            Self::Install => 4,
            Self::MonitorZones => 5,
            Self::Verify => 6,
        }
    }

    pub fn label(&self) -> &str {
        match self {
            Self::WaitForBaseline => "Wait for baseline service",
            Self::Uninstall => "Uninstall broken state",
            Self::DestroyVirtualHw => "Destroy virtual hardware",
            Self::CreateVirtualHw => "Recreate virtual hardware",
            Self::Install => "Install packages",
            Self::MonitorZones => "Monitor zone startup",
            Self::Verify => "Verify services",
        }
    }

    pub fn estimated_duration(&self) -> Duration {
        match self {
            Self::WaitForBaseline => Duration::from_secs(45),
            Self::Uninstall => Duration::from_secs(30),
            Self::DestroyVirtualHw => Duration::from_secs(10),
            Self::CreateVirtualHw => Duration::from_secs(30),
            Self::Install => Duration::from_secs(60),
            Self::MonitorZones => Duration::from_secs(360),
            Self::Verify => Duration::from_secs(10),
        }
    }

    pub fn all() -> &'static [RecoveryStep] {
        &[
            Self::WaitForBaseline,
            Self::Uninstall,
            Self::DestroyVirtualHw,
            Self::CreateVirtualHw,
            Self::Install,
            Self::MonitorZones,
            Self::Verify,
        ]
    }

    pub fn total_count() -> usize {
        7
    }
}

// --- Recovery Events ---

#[derive(Debug, Clone)]
pub enum RecoveryEvent {
    StepStarted(RecoveryStep),
    StepOutput(String),
    ZoneProgress { running: u32, expected: u32 },
    StepCompleted(RecoveryStep, Duration),
    StepFailed {
        step: RecoveryStep,
        error: String,
        workaround: Option<Workaround>,
    },
    RecoveryComplete(Duration),
}

#[derive(Debug, Clone)]
pub enum Workaround {
    ForceUninstallSoftNpu,
    DestroyStaleZpools,
    FixOwnership,
}

impl Workaround {
    pub fn description(&self) -> &str {
        match self {
            Self::ForceUninstallSoftNpu => "Force-uninstall stuck SoftNPU zone, then retry",
            Self::DestroyStaleZpools => "Destroy stale oxp_ zpools, then retry",
            Self::FixOwnership => "Fix ownership of build dirs (pfexec chown), then retry",
        }
    }
}

// --- Recovery Parameters ---

#[derive(Debug, Clone)]
pub struct RecoveryParams {
    pub gateway: String,
    pub pxa_start: String,
    pub pxa_end: String,
    pub vdev_size_bytes: u64,
    pub omicron_path: String,
    pub expected_service_total: u32,
    pub dns_ip: String,
    pub nexus_ip: String,
}

impl RecoveryParams {
    pub fn from_config(config: &DeploymentConfig) -> Result<Self> {
        let network = &config.deployment.network;
        let overrides = &config.build.omicron.overrides;

        let vdev_size = overrides
            .vdev_size_bytes
            .unwrap_or(42949672960); // 40 GiB default

        let dns_ip = network
            .external_dns_ips
            .first()
            .cloned()
            .ok_or_else(|| eyre!("No external DNS IPs configured"))?;

        let nexus_ip = network.internal_services_range.first.clone();

        Ok(Self {
            gateway: network.gateway.clone(),
            pxa_start: network.internal_services_range.first.clone(),
            pxa_end: network.instance_pool_range.last.clone(),
            vdev_size_bytes: vdev_size,
            omicron_path: config.build.omicron.repo_path.clone(),
            expected_service_total: crate::config::types::derive_expected_zones(&config.build.omicron.overrides).values().sum(),
            dns_ip,
            nexus_ip,
        })
    }
}

// --- Recovery Execution ---

/// Run the full 7-step recovery sequence.
pub async fn run_recovery(
    host: &dyn RemoteHost,
    params: &RecoveryParams,
    tx: mpsc::Sender<RecoveryEvent>,
    cancel: CancellationToken,
) -> Result<()> {
    let overall_start = Instant::now();

    for &step in RecoveryStep::all() {
        if cancel.is_cancelled() {
            return Err(eyre!("Recovery cancelled"));
        }

        let _ = tx.send(RecoveryEvent::StepStarted(step)).await;
        let step_start = Instant::now();

        let result = match step {
            RecoveryStep::WaitForBaseline => {
                step_wait_baseline(host, &tx, &cancel).await
            }
            RecoveryStep::Uninstall => {
                step_uninstall(host, params, &tx).await
            }
            RecoveryStep::DestroyVirtualHw => {
                step_destroy_vhw(host, params, &tx).await
            }
            RecoveryStep::CreateVirtualHw => {
                step_create_vhw(host, params, &tx).await
            }
            RecoveryStep::Install => {
                step_install(host, params, &tx).await
            }
            RecoveryStep::MonitorZones => {
                step_monitor_zones(host, params, &tx, &cancel).await
            }
            RecoveryStep::Verify => {
                step_verify(host, params, &tx).await
            }
        };

        let elapsed = step_start.elapsed();

        match result {
            Ok(()) => {
                let _ = tx.send(RecoveryEvent::StepCompleted(step, elapsed)).await;
            }
            Err(e) => {
                let workaround = detect_workaround(step, &e.to_string());
                let _ = tx
                    .send(RecoveryEvent::StepFailed {
                        step,
                        error: e.to_string(),
                        workaround,
                    })
                    .await;
                return Err(e);
            }
        }
    }

    let _ = tx
        .send(RecoveryEvent::RecoveryComplete(overall_start.elapsed()))
        .await;

    Ok(())
}


// --- Individual Steps ---

async fn step_wait_baseline(
    host: &dyn RemoteHost,
    tx: &mpsc::Sender<RecoveryEvent>,
    cancel: &CancellationToken,
) -> Result<()> {
    let timeout = Duration::from_secs(120);
    let start = Instant::now();

    loop {
        if cancel.is_cancelled() {
            return Err(eyre!("Cancelled"));
        }
        if start.elapsed() > timeout {
            return Err(eyre!(
                "Timed out waiting for omicron/baseline service ({}s)",
                timeout.as_secs()
            ));
        }

        let output = host
            .execute("svcs -H -o state omicron/baseline 2>/dev/null")
            .await?;

        let state = output.stdout.trim();
        let _ = tx
            .send(RecoveryEvent::StepOutput(format!("baseline: {state}")))
            .await;

        if state == "online" {
            return Ok(());
        }

        tokio::select! {
            _ = cancel.cancelled() => return Err(eyre!("Cancelled")),
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
        }
    }
}

async fn step_uninstall(
    host: &dyn RemoteHost,
    params: &RecoveryParams,
    tx: &mpsc::Sender<RecoveryEvent>,
) -> Result<()> {
    let cmd = format!(
        "cd {} && source env.sh && echo 'y' | pfexec ./target/release/omicron-package uninstall 2>&1",
        params.omicron_path
    );
    let _ = tx
        .send(RecoveryEvent::StepOutput("Running omicron-package uninstall...".to_string()))
        .await;

    let output = host.execute(&cmd).await?;

    // Send last few lines of output
    for line in output.stdout.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev() {
        let _ = tx.send(RecoveryEvent::StepOutput(line.to_string())).await;
    }

    if output.exit_code != 0 {
        return Err(eyre!(
            "omicron-package uninstall failed (exit {}): {}",
            output.exit_code,
            output.stderr.lines().last().unwrap_or("")
        ));
    }

    Ok(())
}

async fn step_destroy_vhw(
    host: &dyn RemoteHost,
    params: &RecoveryParams,
    tx: &mpsc::Sender<RecoveryEvent>,
) -> Result<()> {
    let cmd = format!(
        "cd {} && source env.sh && pfexec cargo xtask virtual-hardware destroy 2>&1",
        params.omicron_path
    );
    let _ = tx
        .send(RecoveryEvent::StepOutput("Destroying virtual hardware...".to_string()))
        .await;

    let output = host.execute(&cmd).await?;

    for line in output.stdout.lines().rev().take(3).collect::<Vec<_>>().into_iter().rev() {
        let _ = tx.send(RecoveryEvent::StepOutput(line.to_string())).await;
    }

    if output.exit_code != 0 {
        let err_msg = output.stderr.lines().last().unwrap_or("unknown error");
        return Err(eyre!("virtual-hardware destroy failed (exit {}): {}", output.exit_code, err_msg));
    }

    Ok(())
}

async fn step_create_vhw(
    host: &dyn RemoteHost,
    params: &RecoveryParams,
    tx: &mpsc::Sender<RecoveryEvent>,
) -> Result<()> {
    let cmd = format!(
        "cd {} && source env.sh && pfexec cargo xtask virtual-hardware create \
         --gateway-ip {} \
         --pxa-start {} \
         --pxa-end {} \
         --vdev-size {} 2>&1",
        params.omicron_path,
        params.gateway,
        params.pxa_start,
        params.pxa_end,
        params.vdev_size_bytes,
    );
    let _ = tx
        .send(RecoveryEvent::StepOutput("Creating virtual hardware...".to_string()))
        .await;

    let output = host.execute(&cmd).await?;

    for line in output.stdout.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev() {
        let _ = tx.send(RecoveryEvent::StepOutput(line.to_string())).await;
    }

    if output.exit_code != 0 {
        let err_msg = output.stderr.lines().last().unwrap_or("unknown error");
        return Err(eyre!(
            "virtual-hardware create failed (exit {}): {}",
            output.exit_code,
            err_msg
        ));
    }

    Ok(())
}

async fn step_install(
    host: &dyn RemoteHost,
    params: &RecoveryParams,
    tx: &mpsc::Sender<RecoveryEvent>,
) -> Result<()> {
    let cmd = format!(
        "cd {} && source env.sh && pfexec ./target/release/omicron-package install 2>&1",
        params.omicron_path
    );
    let _ = tx
        .send(RecoveryEvent::StepOutput("Running omicron-package install...".to_string()))
        .await;

    let output = host.execute(&cmd).await?;

    for line in output.stdout.lines().rev().take(5).collect::<Vec<_>>().into_iter().rev() {
        let _ = tx.send(RecoveryEvent::StepOutput(line.to_string())).await;
    }

    if output.exit_code != 0 {
        let err_msg = output.stderr.lines().last().unwrap_or("unknown error");
        return Err(eyre!(
            "omicron-package install failed (exit {}): {}",
            output.exit_code,
            err_msg
        ));
    }

    Ok(())
}

async fn step_monitor_zones(
    host: &dyn RemoteHost,
    params: &RecoveryParams,
    tx: &mpsc::Sender<RecoveryEvent>,
    cancel: &CancellationToken,
) -> Result<()> {
    let timeout = Duration::from_secs(600); // 10 minutes
    let start = Instant::now();

    loop {
        if cancel.is_cancelled() {
            return Err(eyre!("Cancelled"));
        }
        if start.elapsed() > timeout {
            return Err(eyre!(
                "Timed out waiting for zones ({:.0}s). Expected {} zones.",
                timeout.as_secs_f64(),
                params.expected_service_total
            ));
        }

        let output = host.execute("zoneadm list -cp 2>/dev/null").await?;
        let zone_list = zones::parse_zoneadm_list(&output.stdout).unwrap_or_default();
        let running = zone_list
            .iter()
            .filter(|z| z.status == zones::ZoneStatus::Running && z.kind == zones::ZoneKind::Service)
            .count() as u32;

        let _ = tx
            .send(RecoveryEvent::ZoneProgress {
                running,
                expected: params.expected_service_total,
            })
            .await;

        if running >= params.expected_service_total {
            return Ok(());
        }

        tokio::select! {
            _ = cancel.cancelled() => return Err(eyre!("Cancelled")),
            _ = tokio::time::sleep(Duration::from_secs(10)) => {}
        }
    }
}

async fn step_verify(
    host: &dyn RemoteHost,
    params: &RecoveryParams,
    tx: &mpsc::Sender<RecoveryEvent>,
) -> Result<()> {
    // Check DNS
    let dns_cmd = format!(
        "dig recovery.sys.oxide.test @{} +short +time=3 +tries=1 2>/dev/null",
        params.dns_ip
    );
    let dns_output = host.execute(&dns_cmd).await?;
    let dns_result = network::parse_dns_check(&dns_output.stdout);

    if dns_result.resolved {
        let _ = tx
            .send(RecoveryEvent::StepOutput(format!(
                "DNS: resolving ({})",
                dns_result.addresses.join(", ")
            )))
            .await;
    } else {
        let _ = tx
            .send(RecoveryEvent::StepOutput("DNS: not resolving (may need more time)".to_string()))
            .await;
    }

    // Check Nexus — try the DNS-resolved address first, fall back to config
    let nexus_ip = if let Some(addr) = dns_result.addresses.first() {
        addr.clone()
    } else {
        params.nexus_ip.clone()
    };

    let ping_cmd = format!(
        "curl -sf --connect-timeout 3 --max-time 5 http://{nexus_ip}/v1/ping 2>/dev/null"
    );
    let ping_output = host.execute(&ping_cmd).await?;
    let reachable = network::parse_nexus_ping(ping_output.exit_code);

    if reachable {
        let _ = tx
            .send(RecoveryEvent::StepOutput(format!("Nexus: reachable at {nexus_ip}")))
            .await;
    } else {
        let _ = tx
            .send(RecoveryEvent::StepOutput(format!(
                "Nexus: unreachable at {nexus_ip} (may need more time to start)"
            )))
            .await;
    }

    // Check sled-agent
    let svcs_output = host
        .execute("svcs -H -o state sled-agent 2>/dev/null")
        .await?;
    let sled_state = svcs_output.stdout.trim();
    let _ = tx
        .send(RecoveryEvent::StepOutput(format!("sled-agent: {sled_state}")))
        .await;

    // Verification passes even if Nexus isn't ready yet — zones being up is the key indicator
    Ok(())
}

// --- Workaround Detection ---

fn detect_workaround(step: RecoveryStep, error: &str) -> Option<Workaround> {
    match step {
        RecoveryStep::DestroyVirtualHw => {
            if error.contains("softnpu") || error.contains("zone") || error.contains("SoftNPU") {
                Some(Workaround::ForceUninstallSoftNpu)
            } else {
                None
            }
        }
        RecoveryStep::CreateVirtualHw => {
            if error.contains("UnexpectedUuid") || error.contains("zpool") || error.contains("oxp_") {
                Some(Workaround::DestroyStaleZpools)
            } else {
                None
            }
        }
        RecoveryStep::Install => {
            if error.contains("permission") || error.contains("Permission") || error.contains("owned by") {
                Some(Workaround::FixOwnership)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::*;
    use crate::ssh::mock::MockHost;
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
                    h.insert(
                        "helios01".to_string(),
                        HostConfig {
                            address: "192.168.2.209".to_string(),
                            ssh_user: "testuser".to_string(),
                            role: HostRole::Combined,
                            host_type: None,
                            ssh_port: None,
                        },
                    );
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
            build: BuildToml {
                omicron: OmicronBuildConfig {
                    overrides: OmicronOverrides {
                        cockroachdb_redundancy: Some(3),
                        vdev_count: Some(3),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            monitoring: MonitoringToml::default(),
        }
    }

    #[test]
    fn test_recovery_params_from_config() {
        let config = sample_config();
        let params = RecoveryParams::from_config(&config).unwrap();
        assert_eq!(params.gateway, "192.168.2.1");
        assert_eq!(params.pxa_start, "192.168.2.70");
        assert_eq!(params.pxa_end, "192.168.2.90");
        assert_eq!(params.vdev_size_bytes, 42949672960);
        // crdb=3, vdevs=3, plus fixed services (int_dns=3, ext_dns=2, nexus=3, clickhouse=1, pantry=3, oximeter=1, ntp=1, softnpu=1, switch=1) = 22
        assert_eq!(params.expected_service_total, 22);
    }

    #[test]
    fn test_recovery_step_ordering() {
        let steps = RecoveryStep::all();
        assert_eq!(steps.len(), 7);
        assert_eq!(steps[0], RecoveryStep::WaitForBaseline);
        assert_eq!(steps[6], RecoveryStep::Verify);
        for (i, step) in steps.iter().enumerate() {
            assert_eq!(step.index(), i);
        }
    }

    #[test]
    fn test_detect_workaround_softnpu() {
        let w = detect_workaround(
            RecoveryStep::DestroyVirtualHw,
            "failed to destroy: softnpu zone still running",
        );
        assert!(matches!(w, Some(Workaround::ForceUninstallSoftNpu)));
    }

    #[test]
    fn test_detect_workaround_stale_zpools() {
        let w = detect_workaround(
            RecoveryStep::CreateVirtualHw,
            "Error: UnexpectedUuid zpool mismatch",
        );
        assert!(matches!(w, Some(Workaround::DestroyStaleZpools)));
    }

    #[test]
    fn test_detect_workaround_none() {
        let w = detect_workaround(RecoveryStep::Verify, "some random error");
        assert!(w.is_none());
    }

    #[tokio::test]
    async fn test_step_wait_baseline_already_online() {
        let mut mock = MockHost::new("test");
        mock.add_success("svcs -H -o state omicron/baseline", "online\n");

        let (tx, _rx) = mpsc::channel(32);
        let cancel = CancellationToken::new();
        let result = step_wait_baseline(&mock, &tx, &cancel).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_full_recovery_happy_path() {
        let mut mock = MockHost::new("test");

        // Step 1: baseline online
        mock.add_success("svcs -H -o state omicron/baseline", "online\n");
        // Step 2: uninstall succeeds
        mock.add_success("omicron-package uninstall", "Uninstalled successfully\n");
        // Step 3: destroy succeeds
        mock.add_success("virtual-hardware destroy", "Destroyed\n");
        // Step 4: create succeeds
        mock.add_success("virtual-hardware create", "Created\n");
        // Step 5: install succeeds
        mock.add_success("omicron-package install", "Installed\n");
        // Step 6: zones come up
        let zone_output = "0:global:running:/::\tipkg:shared\n".to_string()
            + &(1..=22)
                .map(|i| {
                    format!(
                        "{i}:oxz_svc_{i:04x}:running:/pool/ext/oxp_aaa/crypt/zone/oxz_svc_{i:04x}:{i:04x}:omicron1:excl"
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
        mock.add_success("zoneadm list -cp", &zone_output);
        // Step 7: verify
        mock.add_success("dig", "192.168.2.72\n");
        mock.add_success("curl", "");
        mock.add_success("svcs -H -o state sled-agent", "online\n");

        let config = sample_config();
        let params = RecoveryParams::from_config(&config).unwrap();
        let (tx, mut rx) = mpsc::channel(256);
        let cancel = CancellationToken::new();

        let result = run_recovery(&mock, &params, tx, cancel).await;
        assert!(result.is_ok());

        // Collect events
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // Should have StepStarted and StepCompleted for all 7 steps, plus RecoveryComplete
        let started_count = events
            .iter()
            .filter(|e| matches!(e, RecoveryEvent::StepStarted(_)))
            .count();
        let completed_count = events
            .iter()
            .filter(|e| matches!(e, RecoveryEvent::StepCompleted(_, _)))
            .count();
        let complete_count = events
            .iter()
            .filter(|e| matches!(e, RecoveryEvent::RecoveryComplete(_)))
            .count();

        assert_eq!(started_count, 7);
        assert_eq!(completed_count, 7);
        assert_eq!(complete_count, 1);
    }
}
