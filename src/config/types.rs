use std::collections::BTreeMap;
use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// --- Global config (~/.whoah/config.toml) ---

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalConfig {
    pub default_deployment: Option<String>,
}

// --- Combined deployment view ---

#[derive(Debug, Clone)]
pub struct DeploymentConfig {
    pub deployment: DeploymentToml,
    pub build: BuildToml,
    pub monitoring: MonitoringToml,
}

// --- deployment.toml ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentToml {
    pub deployment: DeploymentMeta,
    pub hosts: BTreeMap<String, HostConfig>,
    pub network: NetworkConfig,
    #[serde(default)]
    pub nexus: NexusConfig,
    #[serde(default)]
    pub hypervisor: Option<HypervisorRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentMeta {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostConfig {
    pub address: String,
    pub ssh_user: String,
    #[serde(default = "default_role")]
    pub role: HostRole,
    #[serde(default)]
    pub host_type: Option<HostType>,
    #[serde(default)]
    pub ssh_port: Option<u16>,
}

impl HostConfig {
    /// SSH port, defaulting to 22 when not set.
    pub fn ssh_port(&self) -> u16 {
        self.ssh_port.unwrap_or(22)
    }
}

fn default_role() -> HostRole {
    HostRole::Combined
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HostRole {
    ControlPlane,
    Compute,
    Combined,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HostType {
    Vm,
    BareMetal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub gateway: String,
    pub external_dns_ips: Vec<String>,
    pub internal_services_range: IpRange,
    pub infra_ip: String,
    pub instance_pool_range: IpRange,
    #[serde(default)]
    pub ntp_servers: Option<Vec<String>>,
    #[serde(default)]
    pub dns_servers: Option<Vec<String>>,
    #[serde(default)]
    pub external_dns_zone_name: Option<String>,
    #[serde(default)]
    pub rack_subnet: Option<String>,
    #[serde(default)]
    pub uplink_port_speed: Option<String>,
    #[serde(default)]
    pub allowed_source_ips: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpRange {
    pub first: String,
    pub last: String,
}

// --- Nexus API config ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NexusConfig {
    #[serde(default = "default_silo_name")]
    pub silo_name: String,
    #[serde(default = "default_silo_user")]
    pub username: String,
    #[serde(default = "default_silo_password")]
    pub password: String,
    #[serde(default = "default_pool_name")]
    pub ip_pool_name: String,
    #[serde(default)]
    pub quotas: QuotaConfig,
}

fn default_silo_name() -> String {
    "recovery".to_string()
}
fn default_silo_user() -> String {
    "recovery".to_string()
}
fn default_silo_password() -> String {
    "oxide".to_string()
}
fn default_pool_name() -> String {
    "default".to_string()
}

impl Default for NexusConfig {
    fn default() -> Self {
        Self {
            silo_name: default_silo_name(),
            username: default_silo_user(),
            password: default_silo_password(),
            ip_pool_name: default_pool_name(),
            quotas: QuotaConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaConfig {
    #[serde(default = "default_quota_cpus")]
    pub cpus: u64,
    #[serde(default = "default_quota_large")]
    pub memory: u64,
    #[serde(default = "default_quota_large")]
    pub storage: u64,
}

fn default_quota_cpus() -> u64 {
    9999999999
}
fn default_quota_large() -> u64 {
    999999999999999999
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            cpus: default_quota_cpus(),
            memory: default_quota_large(),
            storage: default_quota_large(),
        }
    }
}

// --- Proxmox provisioning ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxmoxConfig {
    pub host: String,
    #[serde(default = "default_proxmox_ssh_user")]
    pub ssh_user: String,
    #[serde(default)]
    pub ssh_port: Option<u16>,
    #[serde(default = "default_proxmox_node")]
    pub node: String,
    #[serde(default = "default_iso_storage")]
    pub iso_storage: String,
    #[serde(default = "default_disk_storage")]
    pub disk_storage: String,
    #[serde(default = "default_iso_file")]
    pub iso_file: String,
    #[serde(default)]
    pub vm: ProxmoxVmConfig,
}

impl ProxmoxConfig {
    /// SSH port, defaulting to 22 when not set.
    pub fn ssh_port(&self) -> u16 {
        self.ssh_port.unwrap_or(22)
    }
}

fn default_proxmox_ssh_user() -> String {
    "root".to_string()
}
fn default_proxmox_node() -> String {
    "PVE".to_string()
}
fn default_iso_storage() -> String {
    "local".to_string()
}
fn default_disk_storage() -> String {
    "local-lvm".to_string()
}
fn default_iso_file() -> String {
    "helios-install-vga.iso".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxmoxVmConfig {
    #[serde(default = "default_vmid")]
    pub vmid: u32,
    #[serde(default = "default_vm_name")]
    pub name: String,
    #[serde(default = "default_cores")]
    pub cores: u32,
    #[serde(default = "default_sockets")]
    pub sockets: u32,
    #[serde(default = "default_memory_mb")]
    pub memory_mb: u32,
    #[serde(default = "default_disk_gb")]
    pub disk_gb: u32,
    #[serde(default = "default_disk_bus")]
    pub disk_bus: String,
    #[serde(default = "default_cpu_type")]
    pub cpu_type: String,
    #[serde(default = "default_os_type")]
    pub os_type: String,
    #[serde(default = "default_net_model")]
    pub net_model: String,
    #[serde(default = "default_net_bridge")]
    pub net_bridge: String,
}

fn default_vmid() -> u32 {
    302
}
fn default_vm_name() -> String {
    "helios02".to_string()
}
fn default_cores() -> u32 {
    2
}
fn default_sockets() -> u32 {
    2
}
fn default_memory_mb() -> u32 {
    49152
}
fn default_disk_gb() -> u32 {
    256
}
fn default_disk_bus() -> String {
    "sata".to_string()
}
fn default_cpu_type() -> String {
    "host".to_string()
}
fn default_os_type() -> String {
    "solaris".to_string()
}
fn default_net_model() -> String {
    "e1000e".to_string()
}
fn default_net_bridge() -> String {
    "vmbr0".to_string()
}

impl Default for ProxmoxVmConfig {
    fn default() -> Self {
        Self {
            vmid: default_vmid(),
            name: default_vm_name(),
            cores: default_cores(),
            sockets: default_sockets(),
            memory_mb: default_memory_mb(),
            disk_gb: default_disk_gb(),
            disk_bus: default_disk_bus(),
            cpu_type: default_cpu_type(),
            os_type: default_os_type(),
            net_model: default_net_model(),
            net_bridge: default_net_bridge(),
        }
    }
}

// --- Shared hypervisor config (~/.whoah/shared/hypervisors/*.toml) ---

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum HypervisorType {
    Proxmox,
    LinuxKvm,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypervisorConfig {
    pub hypervisor: HypervisorMeta,
    pub credentials: HypervisorCredentials,
    #[serde(default)]
    pub proxmox: Option<ProxmoxHypervisorConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypervisorMeta {
    pub name: String,
    #[serde(rename = "type")]
    pub hypervisor_type: HypervisorType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypervisorCredentials {
    pub host: String,
    pub ssh_user: String,
    #[serde(default)]
    pub ssh_port: Option<u16>,
}

impl HypervisorCredentials {
    /// SSH port, defaulting to 22 when not set.
    pub fn ssh_port(&self) -> u16 {
        self.ssh_port.unwrap_or(22)
    }
}

/// Type-specific Proxmox hypervisor settings (shared across VMs on this host)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxmoxHypervisorConfig {
    #[serde(default = "default_proxmox_node")]
    pub node: String,
    #[serde(default = "default_iso_storage")]
    pub iso_storage: String,
    #[serde(default = "default_disk_storage")]
    pub disk_storage: String,
    #[serde(default = "default_iso_file")]
    pub iso_file: String,
}

// --- Hypervisor reference (per-deployment, points to shared hypervisor) ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HypervisorRef {
    #[serde(rename = "ref")]
    pub hypervisor_ref: String,
    #[serde(default)]
    pub vm: Option<VmConfig>,
}

/// Per-deployment VM config (resources, identity). Used when host_type = vm.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmConfig {
    pub vmid: u32,
    pub name: String,
    #[serde(default = "default_cores")]
    pub cores: u32,
    #[serde(default = "default_sockets")]
    pub sockets: u32,
    #[serde(default = "default_memory_mb")]
    pub memory_mb: u32,
    #[serde(default = "default_disk_gb")]
    pub disk_gb: u32,
    #[serde(default = "default_disk_bus")]
    pub disk_bus: String,
    #[serde(default = "default_cpu_type")]
    pub cpu_type: String,
    #[serde(default = "default_os_type")]
    pub os_type: String,
    #[serde(default = "default_net_model")]
    pub net_model: String,
    #[serde(default = "default_net_bridge")]
    pub net_bridge: String,
}

impl Default for VmConfig {
    fn default() -> Self {
        Self {
            vmid: 100,
            name: String::new(),
            cores: default_cores(),
            sockets: default_sockets(),
            memory_mb: default_memory_mb(),
            disk_gb: default_disk_gb(),
            disk_bus: default_disk_bus(),
            cpu_type: default_cpu_type(),
            os_type: default_os_type(),
            net_model: default_net_model(),
            net_bridge: default_net_bridge(),
        }
    }
}

// --- build.toml ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildToml {
    pub omicron: OmicronBuildConfig,
    #[serde(default)]
    pub propolis: Option<PropolisBuildConfig>,
    #[serde(default)]
    pub tuning: TuningConfig,
}

impl Default for BuildToml {
    fn default() -> Self {
        Self {
            omicron: OmicronBuildConfig::default(),
            propolis: None,
            tuning: TuningConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmicronBuildConfig {
    #[serde(default = "default_omicron_repo_path")]
    pub repo_path: String,
    #[serde(default)]
    pub repo_url: Option<String>,
    #[serde(default)]
    pub git_ref: Option<String>,
    #[serde(default)]
    pub rust_toolchain: Option<String>,
    #[serde(default)]
    pub overrides: OmicronOverrides,
}

fn default_omicron_repo_path() -> String {
    "~/omicron".to_string()
}

impl Default for OmicronBuildConfig {
    fn default() -> Self {
        Self {
            repo_path: default_omicron_repo_path(),
            repo_url: None,
            git_ref: None,
            rust_toolchain: None,
            overrides: OmicronOverrides::default(),
        }
    }
}

/// Derive expected zone counts from build overrides.
/// CockroachDB and Crucible counts come from the overrides;
/// everything else is a fixed default per the Omicron non-gimlet docs.
pub fn derive_expected_zones(overrides: &OmicronOverrides) -> HashMap<String, u32> {
    let crdb = overrides.cockroachdb_redundancy.unwrap_or(5);
    let vdevs = overrides.vdev_count.unwrap_or(9);

    let mut expected = HashMap::new();
    expected.insert("cockroachdb".into(), crdb);
    expected.insert("crucible".into(), vdevs);
    expected.insert("internal_dns".into(), 3);
    expected.insert("external_dns".into(), 2);
    expected.insert("nexus".into(), 3);
    expected.insert("clickhouse".into(), 1);
    expected.insert("crucible_pantry".into(), 3);
    expected.insert("oximeter".into(), 1);
    expected.insert("ntp".into(), 1);
    expected.insert("sidecar_softnpu".into(), 1);
    expected.insert("switch".into(), 1);
    expected
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OmicronOverrides {
    pub cockroachdb_redundancy: Option<u32>,
    pub control_plane_storage_buffer_gib: Option<u32>,
    pub vdev_count: Option<u32>,
    pub vdev_size_bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PropolisBuildConfig {
    #[serde(default = "default_propolis_repo_path")]
    pub repo_path: String,
    #[serde(default)]
    pub patched: Option<bool>,
    #[serde(default)]
    pub patch_type: Option<String>,
    #[serde(default)]
    pub source: Option<PropolisSource>,
    #[serde(default)]
    pub repo_url: Option<String>,
    #[serde(default)]
    pub git_ref: Option<String>,
    #[serde(default)]
    pub local_binary: Option<String>,
}

fn default_propolis_repo_path() -> String {
    "~/propolis".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PropolisSource {
    GithubRelease,
    LocalBuild,
}

// --- Tuning config (in build.toml) ---

/// Advanced tuning options for sled-agent, compile-time flags, and system config.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TuningConfig {
    /// Enable svcadm_autoclear compile-time flag (auto-clear maintenance services)
    #[serde(default)]
    pub svcadm_autoclear: Option<bool>,
    /// Swap zvol size in GB created during OS setup (default: 4)
    #[serde(default)]
    pub swap_size_gb: Option<u32>,
    /// Custom vdev directory (default: /var/tmp via xtask)
    #[serde(default)]
    pub vdev_dir: Option<String>,
    /// Control plane memory earmark in MB (sled-agent config, default: 6144)
    #[serde(default)]
    pub memory_earmark_mb: Option<u32>,
    /// VMM reservoir percentage of unbudgeted DRAM (sled-agent config, default: 60)
    #[serde(default)]
    pub vmm_reservoir_percentage: Option<u32>,
    /// Swap device size in GB for sled-agent config (default: 64)
    #[serde(default)]
    pub swap_device_size_gb: Option<u32>,
}

// --- state.toml (per-deployment runtime state) ---

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeploymentState {
    #[serde(default)]
    pub drift: Option<DriftState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DriftState {
    pub last_checked: String,
}

// --- monitoring.toml ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitoringToml {
    #[serde(default)]
    pub thresholds: Thresholds,
    #[serde(default)]
    pub polling: PollingConfig,
}

impl Default for MonitoringToml {
    fn default() -> Self {
        Self {
            thresholds: Thresholds::default(),
            polling: PollingConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thresholds {
    #[serde(default = "default_rpool_warning")]
    pub rpool_warning_percent: u8,
    #[serde(default = "default_rpool_critical")]
    pub rpool_critical_percent: u8,
    #[serde(default = "default_vdev_warning")]
    pub vdev_warning_gib: u32,
    #[serde(default = "default_oxp_warning")]
    pub oxp_pool_warning_percent: u8,
}

fn default_rpool_warning() -> u8 {
    85
}
fn default_rpool_critical() -> u8 {
    92
}
fn default_vdev_warning() -> u32 {
    35
}
fn default_oxp_warning() -> u8 {
    85
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            rpool_warning_percent: default_rpool_warning(),
            rpool_critical_percent: default_rpool_critical(),
            vdev_warning_gib: default_vdev_warning(),
            oxp_pool_warning_percent: default_oxp_warning(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PollingConfig {
    #[serde(default = "default_status_interval")]
    pub status_interval_secs: u64,
    #[serde(default = "default_disk_interval")]
    pub disk_interval_secs: u64,
}

fn default_status_interval() -> u64 {
    10
}
fn default_disk_interval() -> u64 {
    30
}

impl Default for PollingConfig {
    fn default() -> Self {
        Self {
            status_interval_secs: default_status_interval(),
            disk_interval_secs: default_disk_interval(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_monitoring_defaults() {
        let m = MonitoringToml::default();
        assert_eq!(m.thresholds.rpool_warning_percent, 85);
        assert_eq!(m.thresholds.rpool_critical_percent, 92);
        assert_eq!(m.thresholds.vdev_warning_gib, 35);
        assert_eq!(m.polling.status_interval_secs, 10);
        assert_eq!(m.polling.disk_interval_secs, 30);
    }

    #[test]
    fn test_monitoring_toml_roundtrip() {
        let m = MonitoringToml::default();
        let toml_str = toml::to_string_pretty(&m).unwrap();
        let parsed: MonitoringToml = toml::from_str(&toml_str).unwrap();
        assert_eq!(parsed.thresholds.rpool_warning_percent, 85);
        assert_eq!(parsed.polling.status_interval_secs, 10);
    }

    #[test]
    fn test_host_role_serialization() {
        // TOML can't serialize bare enums; test within a struct
        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            role: HostRole,
        }
        let w = Wrapper {
            role: HostRole::ControlPlane,
        };
        let s = toml::to_string(&w).unwrap();
        assert!(s.contains("control-plane"));
    }

    #[test]
    fn test_build_defaults() {
        let b = BuildToml::default();
        assert_eq!(b.omicron.repo_path, "~/omicron");
        assert!(b.omicron.repo_url.is_none());
        assert!(b.omicron.git_ref.is_none());
        assert!(b.propolis.is_none());
    }

    #[test]
    fn test_hypervisor_type_serialization() {
        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            #[serde(rename = "type")]
            hypervisor_type: HypervisorType,
        }
        let w = Wrapper {
            hypervisor_type: HypervisorType::LinuxKvm,
        };
        let s = toml::to_string(&w).unwrap();
        assert!(s.contains("linux-kvm"));

        let w2 = Wrapper {
            hypervisor_type: HypervisorType::Proxmox,
        };
        let s2 = toml::to_string(&w2).unwrap();
        assert!(s2.contains("proxmox"));
    }

    #[test]
    fn test_host_type_serialization() {
        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            host_type: HostType,
        }
        let w = Wrapper {
            host_type: HostType::Vm,
        };
        let s = toml::to_string(&w).unwrap();
        assert!(s.contains("vm"));

        let w2 = Wrapper {
            host_type: HostType::BareMetal,
        };
        let s2 = toml::to_string(&w2).unwrap();
        assert!(s2.contains("bare-metal"));
    }

    #[test]
    fn test_propolis_source_serialization() {
        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            source: PropolisSource,
        }
        let w = Wrapper {
            source: PropolisSource::GithubRelease,
        };
        let s = toml::to_string(&w).unwrap();
        assert!(s.contains("github-release"));

        let w2 = Wrapper {
            source: PropolisSource::LocalBuild,
        };
        let s2 = toml::to_string(&w2).unwrap();
        assert!(s2.contains("local-build"));
    }

    #[test]
    fn test_hypervisor_config_roundtrip() {
        let toml_str = r#"
[hypervisor]
name = "proxmox-lab"
type = "proxmox"

[credentials]
host = "192.168.2.5"
ssh_user = "root"

[proxmox]
node = "PVE"
iso_storage = "local"
disk_storage = "local-lvm"
iso_file = "helios-install-vga.iso"
"#;
        let config: HypervisorConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.hypervisor.name, "proxmox-lab");
        assert_eq!(config.hypervisor.hypervisor_type, HypervisorType::Proxmox);
        assert_eq!(config.credentials.host, "192.168.2.5");
        assert_eq!(config.credentials.ssh_user, "root");
        let px = config.proxmox.clone().unwrap();
        assert_eq!(px.node, "PVE");
        assert_eq!(px.iso_file, "helios-install-vga.iso");

        // Roundtrip
        let serialized = toml::to_string_pretty(&config).unwrap();
        let reparsed: HypervisorConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(reparsed.hypervisor.name, "proxmox-lab");
    }

    #[test]
    fn test_hypervisor_ref_roundtrip() {
        let toml_str = r#"
ref = "proxmox-lab"

[vm]
vmid = 302
name = "helios02"
cores = 2
sockets = 2
memory_mb = 49152
disk_gb = 256
"#;
        let href: HypervisorRef = toml::from_str(toml_str).unwrap();
        assert_eq!(href.hypervisor_ref, "proxmox-lab");
        let vm = href.vm.unwrap();
        assert_eq!(vm.vmid, 302);
        assert_eq!(vm.name, "helios02");
        assert_eq!(vm.cores, 2);
        assert_eq!(vm.memory_mb, 49152);
    }

    #[test]
    fn test_deployment_state_roundtrip() {
        let state = DeploymentState {
            drift: Some(DriftState {
                last_checked: "2026-03-18T14:30:00Z".to_string(),
            }),
        };
        let s = toml::to_string_pretty(&state).unwrap();
        let parsed: DeploymentState = toml::from_str(&s).unwrap();
        assert_eq!(
            parsed.drift.unwrap().last_checked,
            "2026-03-18T14:30:00Z"
        );

        // Empty state
        let empty = DeploymentState::default();
        assert!(empty.drift.is_none());
    }

    #[test]
    fn test_backward_compat_build_toml() {
        let toml_str = r#"
[omicron]
repo_path = "~/omicron"
rust_toolchain = "1.91.1"

[omicron.overrides]
cockroachdb_redundancy = 3
control_plane_storage_buffer_gib = 5
vdev_count = 3
vdev_size_bytes = 42949672960
"#;
        let config: BuildToml = toml::from_str(toml_str).unwrap();
        assert_eq!(config.omicron.repo_path, "~/omicron");
        assert!(config.omicron.repo_url.is_none()); // new field absent = None
        assert!(config.omicron.git_ref.is_none()); // new field absent = None
        assert!(config.propolis.is_none());
    }

    #[test]
    fn test_expanded_build_toml() {
        let toml_str = r#"
[omicron]
repo_path = "~/omicron"
repo_url = "https://github.com/oxidecomputer/omicron.git"
git_ref = "release-v12"
rust_toolchain = "1.91.1"

[omicron.overrides]
cockroachdb_redundancy = 3
vdev_count = 3

[propolis]
repo_path = "~/propolis"
patched = true
patch_type = "string-io-emulation"
source = "github-release"
repo_url = "https://github.com/swherdman/propolis"
"#;
        let config: BuildToml = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.omicron.repo_url.as_deref(),
            Some("https://github.com/oxidecomputer/omicron.git")
        );
        assert_eq!(config.omicron.git_ref.as_deref(), Some("release-v12"));
        let p = config.propolis.unwrap();
        assert_eq!(p.patched, Some(true));
        assert_eq!(p.patch_type.as_deref(), Some("string-io-emulation"));
        assert_eq!(p.source, Some(PropolisSource::GithubRelease));
        assert!(p.local_binary.is_none());
    }

    #[test]
    fn test_new_format_deployment_toml() {
        // The new format uses [hypervisor] ref instead of inline [proxmox]
        let toml_str = r#"
[deployment]
name = "helios01"
description = "Single-sled Helios on Proxmox"

[hosts.helios01]
address = "192.168.2.209"
ssh_user = "swherdman"
role = "combined"
host_type = "vm"

[network]
gateway = "192.168.2.1"
external_dns_ips = ["192.168.2.40", "192.168.2.41"]
infra_ip = "192.168.2.50"
internal_services_range = { first = "192.168.2.40", last = "192.168.2.49" }
instance_pool_range = { first = "192.168.2.51", last = "192.168.2.60" }

[hypervisor]
ref = "proxmox-lab"

[hypervisor.vm]
vmid = 301
name = "helios01"
cores = 2
sockets = 2
memory_mb = 49152
disk_gb = 256
"#;
        let config: DeploymentToml = toml::from_str(toml_str).unwrap();
        assert_eq!(config.deployment.name, "helios01");
        let href = config.hypervisor.unwrap();
        assert_eq!(href.hypervisor_ref, "proxmox-lab");
        let vm = href.vm.unwrap();
        assert_eq!(vm.vmid, 301);
        assert_eq!(vm.name, "helios01");
        assert_eq!(vm.cores, 2);
        let host = config.hosts.get("helios01").unwrap();
        assert_eq!(host.host_type, Some(HostType::Vm));
    }

    #[test]
    fn test_host_config_ssh_port_default() {
        let toml_str = r#"
address = "192.168.2.209"
ssh_user = "swherdman"
role = "combined"
"#;
        let config: HostConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.ssh_port, None);
        assert_eq!(config.ssh_port(), 22);
    }

    #[test]
    fn test_host_config_ssh_port_custom() {
        let toml_str = r#"
address = "localhost"
ssh_user = "swherdman"
role = "combined"
ssh_port = 2222
"#;
        let config: HostConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.ssh_port, Some(2222));
        assert_eq!(config.ssh_port(), 2222);
    }

    #[test]
    fn test_host_config_ssh_port_roundtrip() {
        let config = HostConfig {
            address: "localhost".to_string(),
            ssh_user: "user".to_string(),
            role: HostRole::Combined,
            host_type: None,
            ssh_port: Some(2222),
        };
        let s = toml::to_string_pretty(&config).unwrap();
        assert!(s.contains("ssh_port = 2222"));
        let parsed: HostConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed.ssh_port(), 2222);
    }

    #[test]
    fn test_host_config_ssh_port_omitted_roundtrip() {
        let config = HostConfig {
            address: "192.168.2.209".to_string(),
            ssh_user: "user".to_string(),
            role: HostRole::Combined,
            host_type: None,
            ssh_port: None,
        };
        let s = toml::to_string_pretty(&config).unwrap();
        assert!(!s.contains("ssh_port"));
        let parsed: HostConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed.ssh_port(), 22);
    }

    #[test]
    fn test_hypervisor_credentials_ssh_port() {
        let toml_str = r#"
[hypervisor]
name = "proxmox-lab"
type = "proxmox"

[credentials]
host = "192.168.2.5"
ssh_user = "root"
ssh_port = 2222

[proxmox]
node = "PVE"
"#;
        let config: HypervisorConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.credentials.ssh_port(), 2222);
    }

    #[test]
    fn test_hypervisor_credentials_ssh_port_default() {
        let toml_str = r#"
[hypervisor]
name = "proxmox-lab"
type = "proxmox"

[credentials]
host = "192.168.2.5"
ssh_user = "root"

[proxmox]
node = "PVE"
"#;
        let config: HypervisorConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.credentials.ssh_port, None);
        assert_eq!(config.credentials.ssh_port(), 22);
    }
}
