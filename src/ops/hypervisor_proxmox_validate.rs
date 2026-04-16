//! Proxmox hypervisor configuration validation via SSH.
//!
//! Queries the Proxmox host to validate node name, storage names,
//! and ISO file existence. Returns per-field status and available
//! values for picker UIs.

use color_eyre::{Result, eyre::eyre};
use serde::Deserialize;

use crate::config::types::ProxmoxHypervisorConfig;
use crate::ssh::oneshot;

/// Per-field validation status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldStatus {
    /// Not yet checked.
    Unknown,
    /// Validation in progress.
    Checking,
    /// Value is valid.
    Valid,
    /// Value is invalid — includes reason.
    Invalid(String),
}

/// Result of validating all Proxmox config fields.
#[derive(Debug, Clone)]
pub struct ProxmoxValidation {
    pub node: FieldStatus,
    pub disk_storage: FieldStatus,
    pub iso_storage: FieldStatus,
    pub iso_file: FieldStatus,

    /// Available node names (for picker).
    pub available_nodes: Vec<String>,
    /// Storage names that support disk images (for picker).
    pub available_disk_storages: Vec<String>,
    /// Storage names that support ISOs (for picker).
    pub available_iso_storages: Vec<String>,
    /// ISO files on the configured iso_storage (for picker).
    pub available_iso_files: Vec<String>,
    /// Path of the iso_storage on disk (for download target).
    pub iso_storage_path: Option<String>,

    /// If the node name needed case-fixing, this is the corrected value.
    pub node_auto_fix: Option<String>,
}

impl ProxmoxValidation {
    pub fn checking() -> Self {
        Self {
            node: FieldStatus::Checking,
            disk_storage: FieldStatus::Checking,
            iso_storage: FieldStatus::Checking,
            iso_file: FieldStatus::Checking,
            available_nodes: Vec::new(),
            available_disk_storages: Vec::new(),
            available_iso_storages: Vec::new(),
            available_iso_files: Vec::new(),
            iso_storage_path: None,
            node_auto_fix: None,
        }
    }
}

// ── Proxmox API response types ─────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct NodeInfo {
    node: String,
    #[allow(dead_code)]
    status: String,
}

#[derive(Debug, Deserialize)]
struct StorageInfo {
    storage: String,
    content: String,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    storage_type: String,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IsoInfo {
    volid: String,
}

/// VM info from `pvesh get /nodes/{node}/qemu`.
#[derive(Debug, Deserialize)]
struct QemuVmInfo {
    vmid: u32,
    name: String,
    #[allow(dead_code)]
    status: String,
}

/// VM config from `pvesh get /nodes/{node}/qemu/{vmid}/config`.
#[derive(Debug, Deserialize)]
struct QemuVmConfig {
    #[serde(default)]
    name: Option<String>,
    #[serde(default = "default_cores")]
    cores: u32,
    #[serde(default = "default_sockets")]
    sockets: u32,
    #[serde(default)]
    memory: Option<String>,
    #[serde(default)]
    cpu: Option<String>,
    #[serde(default)]
    ostype: Option<String>,
    // Disk: could be sata0, scsi0, or ide0
    #[serde(default)]
    sata0: Option<String>,
    #[serde(default)]
    scsi0: Option<String>,
    #[serde(default)]
    ide0: Option<String>,
    // Network
    #[serde(default)]
    net0: Option<String>,
}

fn default_cores() -> u32 {
    2
}
fn default_sockets() -> u32 {
    1
}

// ── Validation logic ───────────────────────────────────────────────────

/// Validate Proxmox configuration by querying the host via SSH.
///
/// Runs 2-3 `pvesh` commands (nodes, storage, optionally ISO content)
/// and returns per-field validation results + available values for pickers.
pub async fn validate_proxmox(
    host: &str,
    user: &str,
    port: u16,
    config: &ProxmoxHypervisorConfig,
) -> ProxmoxValidation {
    let mut result = ProxmoxValidation {
        node: FieldStatus::Unknown,
        disk_storage: FieldStatus::Unknown,
        iso_storage: FieldStatus::Unknown,
        iso_file: FieldStatus::Unknown,
        available_nodes: Vec::new(),
        available_disk_storages: Vec::new(),
        available_iso_storages: Vec::new(),
        available_iso_files: Vec::new(),
        iso_storage_path: None,
        node_auto_fix: None,
    };

    // Query 1: List nodes
    match ssh_command(host, user, port, "pvesh get /nodes --output-format json").await {
        Ok(output) => {
            match serde_json::from_str::<Vec<NodeInfo>>(&output) {
                Ok(nodes) => {
                    result.available_nodes = nodes.iter().map(|n| n.node.clone()).collect();

                    let exact_match = nodes.iter().any(|n| n.node == config.node);
                    if exact_match {
                        result.node = FieldStatus::Valid;
                    } else {
                        // Case-insensitive match — auto-fix
                        let ci_match = nodes
                            .iter()
                            .find(|n| n.node.to_lowercase() == config.node.to_lowercase());
                        if let Some(correct) = ci_match {
                            result.node = FieldStatus::Valid;
                            result.node_auto_fix = Some(correct.node.clone());
                        } else {
                            let available = result.available_nodes.join(", ");
                            result.node =
                                FieldStatus::Invalid(format!("not found (available: {available})"));
                        }
                    }
                }
                Err(e) => {
                    result.node = FieldStatus::Invalid(format!("parse error: {e}"));
                }
            }
        }
        Err(e) => {
            result.node = FieldStatus::Invalid(format!("query failed: {e}"));
            return result;
        }
    }

    // Query 2: List storage
    match ssh_command(host, user, port, "pvesh get /storage --output-format json").await {
        Ok(output) => {
            match serde_json::from_str::<Vec<StorageInfo>>(&output) {
                Ok(storages) => {
                    // Build picker lists
                    for s in &storages {
                        let content_types: Vec<&str> = s.content.split(',').collect();
                        if content_types.contains(&"images") {
                            result.available_disk_storages.push(s.storage.clone());
                        }
                        if content_types.contains(&"iso") {
                            result.available_iso_storages.push(s.storage.clone());
                        }
                    }

                    // Validate disk_storage
                    if let Some(s) = storages.iter().find(|s| s.storage == config.disk_storage) {
                        if s.content.split(',').any(|c| c.trim() == "images") {
                            result.disk_storage = FieldStatus::Valid;
                        } else {
                            result.disk_storage = FieldStatus::Invalid(
                                "exists but doesn't support disk images".into(),
                            );
                        }
                    } else {
                        let available = result.available_disk_storages.join(", ");
                        result.disk_storage =
                            FieldStatus::Invalid(format!("not found (available: {available})"));
                    }

                    // Validate iso_storage
                    if let Some(s) = storages.iter().find(|s| s.storage == config.iso_storage) {
                        if s.content.split(',').any(|c| c.trim() == "iso") {
                            result.iso_storage = FieldStatus::Valid;
                            result.iso_storage_path = s.path.clone();
                        } else {
                            result.iso_storage =
                                FieldStatus::Invalid("exists but doesn't support ISOs".into());
                        }
                    } else {
                        let available = result.available_iso_storages.join(", ");
                        result.iso_storage =
                            FieldStatus::Invalid(format!("not found (available: {available})"));
                    }
                }
                Err(e) => {
                    result.disk_storage = FieldStatus::Invalid(format!("parse error: {e}"));
                    result.iso_storage = FieldStatus::Invalid(format!("parse error: {e}"));
                }
            }
        }
        Err(e) => {
            result.disk_storage = FieldStatus::Invalid(format!("query failed: {e}"));
            result.iso_storage = FieldStatus::Invalid(format!("query failed: {e}"));
            return result;
        }
    }

    // Query 3: List ISO files (only if node + iso_storage are valid)
    let node = result.node_auto_fix.as_deref().unwrap_or(&config.node);

    if result.node == FieldStatus::Valid && result.iso_storage == FieldStatus::Valid {
        let cmd = format!(
            "pvesh get /nodes/{}/storage/{}/content --content iso --output-format json",
            node, config.iso_storage
        );
        match ssh_command(host, user, port, &cmd).await {
            Ok(output) => {
                match serde_json::from_str::<Vec<IsoInfo>>(&output) {
                    Ok(isos) => {
                        result.available_iso_files = isos
                            .iter()
                            .filter_map(|i| {
                                // volid format: "local:iso/helios-install-vga.iso"
                                let name = i.volid.split('/').next_back()?;
                                // Only include Helios install ISOs
                                if name.starts_with("helios-install-") && name.ends_with(".iso") {
                                    Some(name.to_string())
                                } else {
                                    None
                                }
                            })
                            .collect();

                        if result.available_iso_files.contains(&config.iso_file) {
                            result.iso_file = FieldStatus::Valid;
                        } else {
                            result.iso_file = FieldStatus::Invalid("not found on storage".into());
                        }
                    }
                    Err(e) => {
                        result.iso_file = FieldStatus::Invalid(format!("parse error: {e}"));
                    }
                }
            }
            Err(e) => {
                result.iso_file = FieldStatus::Invalid(format!("query failed: {e}"));
            }
        }
    } else {
        result.iso_file = FieldStatus::Invalid("skipped (node or iso_storage invalid)".into());
    }

    result
}

/// Run a command on the Proxmox host via one-shot SSH.
async fn ssh_command(host: &str, user: &str, port: u16, cmd: &str) -> Result<String> {
    let output = oneshot::one_shot(host, user, port, cmd, 30).await?;
    if output.exit_code != 0 {
        return Err(eyre!("Command failed: {}", output.stderr.trim()));
    }
    Ok(output.stdout)
}

// ── VM queries ─────────────────────────────────────────────────────────

/// A VM entry from the Proxmox host, for display in pickers.
#[derive(Debug, Clone)]
pub struct ProxmoxVm {
    pub vmid: u32,
    pub name: String,
}

/// List all VMs on the Proxmox host.
pub async fn list_vms(host: &str, user: &str, port: u16, node: &str) -> Result<Vec<ProxmoxVm>> {
    let cmd = format!("pvesh get /nodes/{node}/qemu --output-format json");
    let output = ssh_command(host, user, port, &cmd).await?;
    let vms: Vec<QemuVmInfo> =
        serde_json::from_str(&output).map_err(|e| eyre!("Failed to parse VM list: {e}"))?;
    Ok(vms
        .iter()
        .map(|v| ProxmoxVm {
            vmid: v.vmid,
            name: v.name.clone(),
        })
        .collect())
}

/// Query a specific VM's config from Proxmox and convert to our VmConfig.
pub async fn query_vm_config(
    host: &str,
    user: &str,
    port: u16,
    node: &str,
    vmid: u32,
) -> Result<crate::config::types::VmConfig> {
    let cmd = format!("pvesh get /nodes/{node}/qemu/{vmid}/config --output-format json");
    let output = ssh_command(host, user, port, &cmd).await?;
    let config: QemuVmConfig =
        serde_json::from_str(&output).map_err(|e| eyre!("Failed to parse VM config: {e}"))?;

    let memory_mb = config
        .memory
        .as_deref()
        .and_then(|m| m.parse::<u32>().ok())
        .unwrap_or(49152);

    // Parse disk size from spec like "local-lvm:vm-301-disk-0,size=256G"
    let (disk_gb, disk_bus) = parse_disk_spec(&config);

    // Parse network from spec like "e1000e=BC:24:11:F4:EF:51,bridge=vmbr0,firewall=1"
    let (net_model, net_bridge) = parse_net_spec(&config);

    Ok(crate::config::types::VmConfig {
        vmid,
        name: config.name.unwrap_or_else(|| format!("vm-{vmid}")),
        cores: config.cores,
        sockets: config.sockets,
        memory_mb,
        disk_gb,
        disk_bus,
        cpu_type: config.cpu.unwrap_or_else(|| "host".into()),
        os_type: config.ostype.unwrap_or_else(|| "solaris".into()),
        net_model,
        net_bridge,
    })
}

/// Parse disk size and bus type from Proxmox VM config.
fn parse_disk_spec(config: &QemuVmConfig) -> (u32, String) {
    let (spec, bus) = if let Some(ref s) = config.sata0 {
        (s.as_str(), "sata")
    } else if let Some(ref s) = config.scsi0 {
        (s.as_str(), "scsi")
    } else if let Some(ref s) = config.ide0 {
        (s.as_str(), "ide")
    } else {
        return (256, "sata".into());
    };

    // Parse "size=256G" from spec like "local-lvm:vm-301-disk-0,size=256G"
    let disk_gb = spec
        .split(',')
        .find_map(|part| {
            let part = part.trim();
            if part.starts_with("size=") {
                let val = part.trim_start_matches("size=").trim_end_matches('G');
                val.parse::<u32>().ok()
            } else {
                None
            }
        })
        .unwrap_or(256);

    (disk_gb, bus.into())
}

/// Parse network model and bridge from Proxmox VM config.
fn parse_net_spec(config: &QemuVmConfig) -> (String, String) {
    let Some(ref net) = config.net0 else {
        return ("e1000e".into(), "vmbr0".into());
    };

    // Format: "e1000e=BC:24:11:F4:EF:51,bridge=vmbr0,firewall=1"
    // Model is before the "="
    let model = net.split('=').next().unwrap_or("e1000e").to_string();

    let bridge = net
        .split(',')
        .find_map(|part| {
            let part = part.trim();
            if part.starts_with("bridge=") {
                Some(part.trim_start_matches("bridge=").to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "vmbr0".into());

    (model, bridge)
}

// Re-export from the shared download module.
pub use crate::ssh::download::DownloadProgress;

/// Download an ISO file to the Proxmox host's ISO storage.
/// Uses the shared `ssh::download::download_remote` function.
pub async fn download_iso(
    host: &str,
    user: &str,
    port: u16,
    iso_storage_path: &str,
    filename: &str,
    progress_tx: tokio::sync::mpsc::Sender<DownloadProgress>,
) -> Result<()> {
    let url = format!("https://pkg.oxide.computer/install/latest/{filename}");
    let dest = format!("{iso_storage_path}/template/iso/{filename}");

    crate::ssh::download::download_remote(host, user, port, &url, &dest, progress_tx).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_nodes_json() {
        let json = r#"[{"node":"pve","status":"online","cpu":0.07}]"#;
        let nodes: Vec<NodeInfo> = serde_json::from_str(json).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node, "pve");
    }

    #[test]
    fn test_parse_storage_json() {
        let json = r#"[
            {"storage":"local-lvm","type":"lvmthin","content":"images,rootdir"},
            {"storage":"local","type":"dir","content":"iso,vztmpl,backup","path":"/var/lib/vz"}
        ]"#;
        let storages: Vec<StorageInfo> = serde_json::from_str(json).unwrap();
        assert_eq!(storages.len(), 2);
        assert_eq!(storages[0].storage, "local-lvm");
        assert!(storages[0].content.contains("images"));
        assert_eq!(storages[1].storage, "local");
        assert!(storages[1].content.contains("iso"));
        assert_eq!(storages[1].path.as_deref(), Some("/var/lib/vz"));
    }

    #[test]
    fn test_parse_iso_json() {
        let json = r#"[
            {"volid":"local:iso/helios-install-vga.iso","content":"iso","format":"iso","size":551346176},
            {"volid":"local:iso/ubuntu.iso","content":"iso","format":"iso","size":1234567890}
        ]"#;
        let isos: Vec<IsoInfo> = serde_json::from_str(json).unwrap();
        assert_eq!(isos.len(), 2);
        let filenames: Vec<&str> = isos
            .iter()
            .filter_map(|i| i.volid.split('/').last())
            .collect();
        assert!(filenames.contains(&"helios-install-vga.iso"));
        assert!(filenames.contains(&"ubuntu.iso"));
    }

    #[test]
    fn test_node_case_insensitive_match() {
        let nodes = vec![NodeInfo {
            node: "pve".into(),
            status: "online".into(),
        }];
        let config_node = "PVE";
        let exact = nodes.iter().any(|n| n.node == config_node);
        assert!(!exact);
        let ci = nodes
            .iter()
            .find(|n| n.node.to_lowercase() == config_node.to_lowercase());
        assert!(ci.is_some());
        assert_eq!(ci.unwrap().node, "pve");
    }

    #[test]
    fn test_storage_classification_by_content() {
        let json = r#"[
            {"storage":"local-lvm","type":"lvmthin","content":"images,rootdir"},
            {"storage":"local","type":"dir","content":"iso,vztmpl,backup","path":"/var/lib/vz"},
            {"storage":"shared","type":"nfs","content":"images,iso","path":"/mnt/shared"}
        ]"#;
        let storages: Vec<StorageInfo> = serde_json::from_str(json).unwrap();

        let disk_storages: Vec<&str> = storages
            .iter()
            .filter(|s| s.content.split(',').any(|c| c.trim() == "images"))
            .map(|s| s.storage.as_str())
            .collect();
        let iso_storages: Vec<&str> = storages
            .iter()
            .filter(|s| s.content.split(',').any(|c| c.trim() == "iso"))
            .map(|s| s.storage.as_str())
            .collect();

        // local-lvm has images, shared has images
        assert_eq!(disk_storages, vec!["local-lvm", "shared"]);
        // local has iso, shared has iso
        assert_eq!(iso_storages, vec!["local", "shared"]);
    }

    #[test]
    fn test_iso_filename_filtering() {
        let filenames = vec![
            "helios-install-vga.iso",
            "helios-install-ttya.iso",
            "ubuntu-24.04.iso",
            "TrueNAS-SCALE.iso",
            "helios-install-ttyb.iso",
        ];
        let filtered: Vec<&&str> = filenames
            .iter()
            .filter(|n| n.starts_with("helios-install-") && n.ends_with(".iso"))
            .collect();
        assert_eq!(filtered.len(), 3);
        assert!(filtered.contains(&&"helios-install-vga.iso"));
        assert!(filtered.contains(&&"helios-install-ttya.iso"));
        assert!(filtered.contains(&&"helios-install-ttyb.iso"));
    }

    #[test]
    fn test_node_exact_match_no_auto_fix() {
        let nodes = vec![NodeInfo {
            node: "pve".into(),
            status: "online".into(),
        }];
        let config_node = "pve";
        let exact = nodes.iter().any(|n| n.node == config_node);
        assert!(exact);
        // No auto-fix needed
        let ci = nodes
            .iter()
            .find(|n| n.node.to_lowercase() == config_node.to_lowercase());
        assert_eq!(ci.unwrap().node, config_node);
    }

    #[test]
    fn test_node_no_match() {
        let nodes = vec![NodeInfo {
            node: "pve".into(),
            status: "online".into(),
        }];
        let config_node = "badnode";
        let exact = nodes.iter().any(|n| n.node == config_node);
        assert!(!exact);
        let ci = nodes
            .iter()
            .find(|n| n.node.to_lowercase() == config_node.to_lowercase());
        assert!(ci.is_none());
    }
}
