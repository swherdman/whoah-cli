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
    pub hosts: HashMap<String, HostConfig>,
    pub network: NetworkConfig,
    #[serde(default)]
    pub nexus: NexusConfig,
    #[serde(default)]
    pub proxmox: Option<ProxmoxConfig>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub gateway: String,
    pub external_dns_ips: Vec<String>,
    pub internal_services_range: IpRange,
    pub infra_ip: String,
    pub instance_pool_range: IpRange,
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

// --- build.toml ---

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildToml {
    pub omicron: OmicronBuildConfig,
    #[serde(default)]
    pub propolis: Option<PropolisBuildConfig>,
}

impl Default for BuildToml {
    fn default() -> Self {
        Self {
            omicron: OmicronBuildConfig::default(),
            propolis: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OmicronBuildConfig {
    #[serde(default = "default_omicron_repo_path")]
    pub repo_path: String,
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
}

fn default_propolis_repo_path() -> String {
    "~/propolis".to_string()
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
        assert!(b.propolis.is_none());
    }
}
