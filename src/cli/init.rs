use std::collections::BTreeMap;

use color_eyre::{eyre::eyre, Result};
use dialoguer::{Confirm, Input};

use super::InitArgs;
use crate::config::editor::{create_deployment, write_global_config};
use crate::config::loader::list_deployments;
use crate::config::types::*;
use crate::ops::import::discover_config;
use crate::ssh::session::SshHost;

pub async fn run(args: InitArgs) -> Result<()> {
    if let Some(host_str) = &args.import {
        run_import(host_str).await
    } else {
        run_wizard().await
    }
}

// --- Import from running host ---

async fn run_import(host_str: &str) -> Result<()> {
    // Parse user@host or just host
    let (ssh_user, address) = if host_str.contains('@') {
        let parts: Vec<&str> = host_str.splitn(2, '@').collect();
        (parts[0].to_string(), parts[1].to_string())
    } else {
        let user: String = Input::new()
            .with_prompt("SSH user")
            .with_initial_text(whoami())
            .interact_text()?;
        (user, host_str.to_string())
    };

    eprintln!("Connecting to {ssh_user}@{address}...");

    let host_config = HostConfig {
        address: address.clone(),
        ssh_user: ssh_user.clone(),
        role: HostRole::Combined,
        host_type: None,
        ssh_port: None,
    };
    let host = SshHost::connect(&host_config).await?;

    eprintln!("Discovering configuration...");
    let discovered = discover_config(&host).await?;

    eprintln!("\nDiscovered configuration:");
    eprintln!("  Network gateway:    {}", discovered.network.gateway);
    eprintln!(
        "  External DNS:       {}",
        discovered.network.external_dns_ips.join(", ")
    );
    eprintln!(
        "  Services range:     {} - {}",
        discovered.network.internal_services_range.first,
        discovered.network.internal_services_range.last
    );
    eprintln!("  Infra IP:           {}", discovered.network.infra_ip);
    eprintln!(
        "  Instance pool:      {} - {}",
        discovered.network.instance_pool_range.first,
        discovered.network.instance_pool_range.last
    );
    eprintln!("  Vdev count:         {}", discovered.vdev_count);
    if let Some(size) = discovered.vdev_size_bytes {
        eprintln!("  Vdev size:          {} GiB", size / 1_073_741_824);
    }
    if let Some(crdb) = discovered.cockroachdb_redundancy {
        eprintln!("  CRDB redundancy:    {crdb}");
    }
    if let Some(buf) = discovered.storage_buffer_gib {
        eprintln!("  Storage buffer:     {buf} GiB");
    }
    eprintln!("  Omicron path:       {}", discovered.omicron_path);

    let name: String = Input::new()
        .with_prompt("\nDeployment name")
        .with_initial_text("helios-lab")
        .interact_text()?;

    let proceed = Confirm::new()
        .with_prompt("Create deployment config?")
        .default(true)
        .interact()?;

    if !proceed {
        eprintln!("Cancelled.");
        host.close().await?;
        return Ok(());
    }

    let config = build_config_from_discovered(
        &name,
        &address,
        &ssh_user,
        &discovered,
    );

    create_deployment(&name, &config)?;
    set_default_if_only(&name)?;

    eprintln!("\nDeployment '{name}' created at ~/.whoah/deployments/{name}/");
    eprintln!("Run 'whoah status' to check the system.");

    host.close().await?;
    Ok(())
}

// --- Manual wizard ---

async fn run_wizard() -> Result<()> {
    eprintln!("whoah — New Deployment Setup\n");

    let name: String = Input::new()
        .with_prompt("Deployment name")
        .with_initial_text("helios-lab")
        .interact_text()?;

    // Check if deployment already exists
    let existing = list_deployments()?;
    if existing.contains(&name) {
        return Err(eyre!(
            "Deployment '{name}' already exists. Remove ~/.whoah/deployments/{name}/ first."
        ));
    }

    let address: String = Input::new()
        .with_prompt("Helios host IP")
        .with_initial_text("192.168.2.209")
        .interact_text()?;

    let ssh_user: String = Input::new()
        .with_prompt("SSH user")
        .with_initial_text(whoami())
        .interact_text()?;

    let gateway: String = Input::new()
        .with_prompt("Gateway IP")
        .with_initial_text("192.168.2.1")
        .interact_text()?;

    let dns_ips: String = Input::new()
        .with_prompt("External DNS IPs (comma-separated)")
        .with_initial_text("192.168.2.70,192.168.2.71")
        .interact_text()?;

    let services_first: String = Input::new()
        .with_prompt("Internal services IP range start")
        .with_initial_text("192.168.2.70")
        .interact_text()?;

    let services_last: String = Input::new()
        .with_prompt("Internal services IP range end")
        .with_initial_text("192.168.2.79")
        .interact_text()?;

    let infra_ip: String = Input::new()
        .with_prompt("Infra IP (SoftNPU)")
        .with_initial_text("192.168.2.80")
        .interact_text()?;

    let pool_first: String = Input::new()
        .with_prompt("Instance pool IP range start")
        .with_initial_text("192.168.2.81")
        .interact_text()?;

    let pool_last: String = Input::new()
        .with_prompt("Instance pool IP range end")
        .with_initial_text("192.168.2.90")
        .interact_text()?;

    let vdev_count: u32 = Input::new()
        .with_prompt("Number of vdevs")
        .default(3)
        .interact_text()?;

    let crdb_redundancy: u32 = Input::new()
        .with_prompt("CockroachDB redundancy")
        .default(3)
        .interact_text()?;

    let storage_buffer: u32 = Input::new()
        .with_prompt("Control plane storage buffer (GiB)")
        .default(5)
        .interact_text()?;

    let config = DeploymentConfig {
        deployment: DeploymentToml {
            deployment: DeploymentMeta {
                name: name.clone(),
                description: Some("Created by whoah init".to_string()),
            },
            hosts: {
                let mut h = BTreeMap::new();
                h.insert(
                    name.clone(),
                    HostConfig {
                        address,
                        ssh_user,
                        role: HostRole::Combined,
                        host_type: None,
                        ssh_port: None,
                    },
                );
                h
            },
            network: NetworkConfig {
                gateway,
                external_dns_ips: dns_ips
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .collect(),
                internal_services_range: IpRange {
                    first: services_first,
                    last: services_last,
                },
                infra_ip,
                instance_pool_range: IpRange {
                    first: pool_first,
                    last: pool_last,
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
                repo_path: "~/omicron".to_string(),
                repo_url: None,
                git_ref: None,
                rust_toolchain: Some("1.91.1".to_string()),
                overrides: OmicronOverrides {
                    cockroachdb_redundancy: Some(crdb_redundancy),
                    control_plane_storage_buffer_gib: Some(storage_buffer),
                    vdev_count: Some(vdev_count),
                    vdev_size_bytes: Some(42949672960),
                },
            },
            propolis: None,
            tuning: Default::default(),
        },
        monitoring: MonitoringToml::default(),
    };

    create_deployment(&name, &config)?;
    set_default_if_only(&name)?;

    eprintln!("\nDeployment '{name}' created at ~/.whoah/deployments/{name}/");
    eprintln!("Run 'whoah status' to check the system.");

    Ok(())
}

// --- Helpers ---

fn build_config_from_discovered(
    name: &str,
    address: &str,
    ssh_user: &str,
    discovered: &crate::ops::import::DiscoveredConfig,
) -> DeploymentConfig {
    DeploymentConfig {
        deployment: DeploymentToml {
            deployment: DeploymentMeta {
                name: name.to_string(),
                description: Some(format!("Imported from {address}")),
            },
            hosts: {
                let mut h = BTreeMap::new();
                h.insert(
                    name.to_string(),
                    HostConfig {
                        address: address.to_string(),
                        ssh_user: ssh_user.to_string(),
                        role: HostRole::Combined,
                        host_type: None,
                        ssh_port: None,
                    },
                );
                h
            },
            network: discovered.network.clone(),
            nexus: NexusConfig::default(),

            hypervisor: None,
        },
        build: BuildToml {
            omicron: OmicronBuildConfig {
                repo_path: discovered.omicron_path.clone(),
                repo_url: None,
                git_ref: None,
                rust_toolchain: Some("1.91.1".to_string()),
                overrides: OmicronOverrides {
                    cockroachdb_redundancy: discovered.cockroachdb_redundancy,
                    control_plane_storage_buffer_gib: discovered.storage_buffer_gib,
                    vdev_count: Some(discovered.vdev_count),
                    vdev_size_bytes: discovered.vdev_size_bytes,
                },
            },
            propolis: None,
            tuning: Default::default(),
        },
        monitoring: MonitoringToml::default(),
    }
}

fn set_default_if_only(name: &str) -> Result<()> {
    let deployments = list_deployments()?;
    if deployments.len() == 1 {
        write_global_config(&GlobalConfig {
            default_deployment: Some(name.to_string()),
        })?;
    }
    Ok(())
}

fn whoami() -> String {
    std::env::var("USER").unwrap_or_else(|_| "root".to_string())
}
