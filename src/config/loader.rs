use std::fs;
use std::path::PathBuf;

use color_eyre::{Result, eyre::eyre};

use super::types::*;

pub fn whoah_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| eyre!("HOME not set"))?;
    Ok(PathBuf::from(home).join(".whoah"))
}

pub fn deployments_dir() -> Result<PathBuf> {
    Ok(whoah_dir()?.join("deployments"))
}

pub fn deployment_dir(name: &str) -> Result<PathBuf> {
    Ok(deployments_dir()?.join(name))
}

/// Create a build log path for a specific deployment.
/// Returns: `~/.whoah/logs/{deployment}/build-{YYYY-MM-DDTHHMM}.log`
/// Creates the directory if it doesn't exist.
pub fn build_log_path(deployment: &str) -> Result<PathBuf> {
    let dir = whoah_dir()?.join("logs").join(deployment);
    fs::create_dir_all(&dir)?;
    let timestamp = chrono::Local::now().format("%Y-%m-%dT%H%M");
    Ok(dir.join(format!("build-{timestamp}.log")))
}

pub fn load_global_config() -> Result<GlobalConfig> {
    let path = whoah_dir()?.join("config.toml");
    if !path.exists() {
        return Ok(GlobalConfig::default());
    }
    let contents = fs::read_to_string(&path)?;
    let config: GlobalConfig = toml::from_str(&contents)?;
    Ok(config)
}

pub fn load_deployment(name: &str) -> Result<DeploymentConfig> {
    let dir = deployment_dir(name)?;
    if !dir.exists() {
        return Err(eyre!(
            "Deployment '{}' not found at {}",
            name,
            dir.display()
        ));
    }

    let deployment_path = dir.join("deployment.toml");
    let build_path = dir.join("build.toml");
    let monitoring_path = dir.join("monitoring.toml");

    let deployment: DeploymentToml = {
        let contents = fs::read_to_string(&deployment_path)
            .map_err(|e| eyre!("Failed to read {}: {}", deployment_path.display(), e))?;
        toml::from_str(&contents)?
    };

    let build: BuildToml = if build_path.exists() {
        let contents = fs::read_to_string(&build_path)?;
        toml::from_str(&contents)?
    } else {
        BuildToml::default()
    };

    let monitoring: MonitoringToml = if monitoring_path.exists() {
        let contents = fs::read_to_string(&monitoring_path)?;
        toml::from_str(&contents)?
    } else {
        MonitoringToml::default()
    };

    Ok(DeploymentConfig {
        deployment,
        build,
        monitoring,
    })
}

pub fn shared_dir() -> Result<PathBuf> {
    Ok(whoah_dir()?.join("shared"))
}

pub fn hypervisors_dir() -> Result<PathBuf> {
    Ok(shared_dir()?.join("hypervisors"))
}

pub fn load_hypervisor(name: &str) -> Result<HypervisorConfig> {
    let path = hypervisors_dir()?.join(format!("{name}.toml"));
    let contents = fs::read_to_string(&path)
        .map_err(|e| eyre!("Hypervisor '{name}' not found at {}: {e}", path.display()))?;
    let config: HypervisorConfig = toml::from_str(&contents)?;
    Ok(config)
}

pub fn list_hypervisors() -> Result<Vec<String>> {
    let dir = hypervisors_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().map(|e| e == "toml").unwrap_or(false)
            && let Some(stem) = path.file_stem().and_then(|s| s.to_str())
        {
            names.push(stem.to_string());
        }
    }
    names.sort();
    Ok(names)
}

/// Find all deployments whose [hypervisor] ref matches the given name.
pub fn find_referencing_deployments(hypervisor_name: &str) -> Result<Vec<String>> {
    let mut refs = Vec::new();
    for name in list_deployments()? {
        let dep_path = deployment_dir(&name)?.join("deployment.toml");
        if let Ok(contents) = fs::read_to_string(&dep_path)
            && let Ok(dep) = toml::from_str::<DeploymentToml>(&contents)
            && let Some(href) = &dep.hypervisor
            && href.hypervisor_ref == hypervisor_name
        {
            refs.push(name);
        }
    }
    refs.sort();
    Ok(refs)
}

pub fn load_deployment_state(name: &str) -> Result<DeploymentState> {
    let path = deployment_dir(name)?.join("state.toml");
    if !path.exists() {
        return Ok(DeploymentState::default());
    }
    let contents = fs::read_to_string(&path)?;
    Ok(toml::from_str(&contents)?)
}

/// Resolve hypervisor ref into a ProxmoxConfig for the deploy pipeline.
pub fn resolve_proxmox_config(deployment: &DeploymentToml) -> Result<Option<ProxmoxConfig>> {
    let Some(href) = &deployment.hypervisor else {
        return Ok(None);
    };
    let hyp = load_hypervisor(&href.hypervisor_ref)?;
    if hyp.hypervisor.hypervisor_type != HypervisorType::Proxmox {
        return Ok(None);
    }
    let px_hyp = hyp.proxmox.ok_or_else(|| {
        eyre!(
            "Hypervisor '{}' is type proxmox but has no [proxmox] section",
            href.hypervisor_ref
        )
    })?;
    let vm = href.vm.as_ref().ok_or_else(|| {
        eyre!(
            "Deployment references hypervisor '{}' but has no [hypervisor.vm] section",
            href.hypervisor_ref
        )
    })?;
    Ok(Some(ProxmoxConfig {
        host: hyp.credentials.host,
        ssh_user: hyp.credentials.ssh_user,
        ssh_port: hyp.credentials.ssh_port,
        node: px_hyp.node,
        iso_storage: px_hyp.iso_storage,
        disk_storage: px_hyp.disk_storage,
        iso_file: px_hyp.iso_file,
        vm: ProxmoxVmConfig {
            vmid: vm.vmid,
            name: vm.name.clone(),
            cores: vm.cores,
            sockets: vm.sockets,
            memory_mb: vm.memory_mb,
            disk_gb: vm.disk_gb,
            disk_bus: vm.disk_bus.clone(),
            cpu_type: vm.cpu_type.clone(),
            os_type: vm.os_type.clone(),
            net_model: vm.net_model.clone(),
            net_bridge: vm.net_bridge.clone(),
        },
    }))
}

pub fn list_deployments() -> Result<Vec<String>> {
    let dir = deployments_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && let Some(name) = entry.file_name().to_str()
        {
            // Only list dirs that have a deployment.toml
            if entry.path().join("deployment.toml").exists() {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

/// Resolve which deployment to use.
/// Priority: explicit name > global default > only deployment > error.
pub fn resolve_deployment(explicit: Option<&str>) -> Result<String> {
    if let Some(name) = explicit {
        return Ok(name.to_string());
    }

    let global = load_global_config()?;
    if let Some(name) = global.default_deployment {
        return Ok(name);
    }

    let deployments = list_deployments()?;
    match deployments.len() {
        0 => Err(eyre!(
            "No deployments found. Run 'whoah init' to create one."
        )),
        1 => deployments
            .into_iter()
            .next()
            .ok_or_else(|| eyre!("unexpected empty list")),
        _ => {
            let selection = dialoguer::Select::new()
                .with_prompt("Multiple deployments found. Select one")
                .items(&deployments)
                .default(0)
                .interact()
                .map_err(|e| eyre!("Failed to select deployment: {e}"))?;
            Ok(deployments[selection].clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn test_resolve_explicit_wins() {
        let result = resolve_deployment(Some("my-lab"));
        assert_eq!(result.unwrap(), "my-lab");
    }

    #[test]
    fn test_deployment_state_default() {
        let state = DeploymentState::default();
        assert!(state.drift.is_none());
    }

    #[test]
    fn test_resolve_proxmox_config_none() {
        // Bare-metal deployment with no hypervisor
        let deployment = DeploymentToml {
            deployment: DeploymentMeta {
                name: "bare".to_string(),
                description: None,
            },
            hosts: BTreeMap::new(),
            network: NetworkConfig {
                gateway: "10.0.0.1".to_string(),
                external_dns_ips: vec![],
                internal_services_range: IpRange {
                    first: "10.0.0.40".to_string(),
                    last: "10.0.0.49".to_string(),
                },
                infra_ip: "10.0.0.50".to_string(),
                instance_pool_range: IpRange {
                    first: "10.0.0.51".to_string(),
                    last: "10.0.0.60".to_string(),
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
        };

        let result = resolve_proxmox_config(&deployment).unwrap();
        assert!(result.is_none());
    }
}
