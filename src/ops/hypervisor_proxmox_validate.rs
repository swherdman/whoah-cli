//! Proxmox hypervisor configuration validation via SSH.
//!
//! Queries the Proxmox host to validate node name, storage names,
//! and ISO file existence. Returns per-field status and available
//! values for picker UIs.

use color_eyre::{eyre::eyre, Result};
use serde::Deserialize;

use crate::config::types::ProxmoxHypervisorConfig;
use crate::ssh::probe::SshProbeStatus;

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

// ── Validation logic ───────────────────────────────────────────────────

/// Validate Proxmox configuration by querying the host via SSH.
///
/// Runs 2-3 `pvesh` commands (nodes, storage, optionally ISO content)
/// and returns per-field validation results + available values for pickers.
pub async fn validate_proxmox(
    host: &str,
    user: &str,
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
    match ssh_command(host, user, "pvesh get /nodes --output-format json").await {
        Ok(output) => {
            match serde_json::from_str::<Vec<NodeInfo>>(&output) {
                Ok(nodes) => {
                    result.available_nodes = nodes.iter().map(|n| n.node.clone()).collect();

                    let exact_match = nodes.iter().any(|n| n.node == config.node);
                    if exact_match {
                        result.node = FieldStatus::Valid;
                    } else {
                        // Case-insensitive match — auto-fix
                        let ci_match = nodes.iter().find(|n| {
                            n.node.to_lowercase() == config.node.to_lowercase()
                        });
                        if let Some(correct) = ci_match {
                            result.node = FieldStatus::Valid;
                            result.node_auto_fix = Some(correct.node.clone());
                        } else {
                            let available = result.available_nodes.join(", ");
                            result.node = FieldStatus::Invalid(format!(
                                "not found (available: {available})"
                            ));
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
    match ssh_command(host, user, "pvesh get /storage --output-format json").await {
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
                        result.disk_storage = FieldStatus::Invalid(format!(
                            "not found (available: {available})"
                        ));
                    }

                    // Validate iso_storage
                    if let Some(s) = storages.iter().find(|s| s.storage == config.iso_storage) {
                        if s.content.split(',').any(|c| c.trim() == "iso") {
                            result.iso_storage = FieldStatus::Valid;
                            result.iso_storage_path = s.path.clone();
                        } else {
                            result.iso_storage = FieldStatus::Invalid(
                                "exists but doesn't support ISOs".into(),
                            );
                        }
                    } else {
                        let available = result.available_iso_storages.join(", ");
                        result.iso_storage = FieldStatus::Invalid(format!(
                            "not found (available: {available})"
                        ));
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
    let node = result
        .node_auto_fix
        .as_deref()
        .unwrap_or(&config.node);

    if result.node == FieldStatus::Valid && result.iso_storage == FieldStatus::Valid {
        let cmd = format!(
            "pvesh get /nodes/{}/storage/{}/content --content iso --output-format json",
            node, config.iso_storage
        );
        match ssh_command(host, user, &cmd).await {
            Ok(output) => {
                match serde_json::from_str::<Vec<IsoInfo>>(&output) {
                    Ok(isos) => {
                        result.available_iso_files = isos
                            .iter()
                            .filter_map(|i| {
                                // volid format: "local:iso/helios-install-vga.iso"
                                i.volid.split('/').last().map(|s| s.to_string())
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
        result.iso_file =
            FieldStatus::Invalid("skipped (node or iso_storage invalid)".into());
    }

    result
}

/// Run a command on the Proxmox host via one-shot SSH (no mux session).
async fn ssh_command(host: &str, user: &str, cmd: &str) -> Result<String> {
    let output = tokio::process::Command::new("ssh")
        .args([
            "-o", "BatchMode=yes",
            "-o", "ConnectTimeout=10",
            "-o", "StrictHostKeyChecking=accept-new",
            "-o", "ControlMaster=no",
            "-o", "ControlPath=none",
            &format!("{user}@{host}"),
            cmd,
        ])
        .output()
        .await
        .map_err(|e| eyre!("SSH command failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(eyre!("Command failed: {}", stderr.trim()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
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
        let nodes = vec![
            NodeInfo { node: "pve".into(), status: "online".into() },
        ];
        let config_node = "PVE";
        let exact = nodes.iter().any(|n| n.node == config_node);
        assert!(!exact);
        let ci = nodes.iter().find(|n| n.node.to_lowercase() == config_node.to_lowercase());
        assert!(ci.is_some());
        assert_eq!(ci.unwrap().node, "pve");
    }
}
