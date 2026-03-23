use std::fs;

use color_eyre::{eyre::eyre, Result};

use super::loader::{deployment_dir, deployments_dir, hypervisors_dir, load_global_config, whoah_dir};
use super::types::*;

pub fn create_deployment(name: &str, config: &DeploymentConfig) -> Result<()> {
    let dir = deployment_dir(name)?;
    fs::create_dir_all(&dir)?;
    fs::create_dir_all(dir.join("state"))?;

    let deployment_toml = toml::to_string_pretty(&config.deployment)?;
    fs::write(dir.join("deployment.toml"), deployment_toml)?;

    let build_toml = toml::to_string_pretty(&config.build)?;
    fs::write(dir.join("build.toml"), build_toml)?;

    let monitoring_toml = toml::to_string_pretty(&config.monitoring)?;
    fs::write(dir.join("monitoring.toml"), monitoring_toml)?;

    Ok(())
}

pub fn write_global_config(config: &GlobalConfig) -> Result<()> {
    let dir = whoah_dir()?;
    fs::create_dir_all(&dir)?;
    let contents = toml::to_string_pretty(config)?;
    fs::write(dir.join("config.toml"), contents)?;
    Ok(())
}

/// Update a single field in a deployment's TOML file, preserving formatting.
/// `file` is "deployment" or "build", `path` is a dotted key like "network.gateway".
/// Handles both regular `[table]` sections and inline tables `{ key = "val" }`.
pub fn update_deployment_field(
    deployment_name: &str,
    file: &str,
    path: &str,
    value: &str,
) -> Result<()> {
    let dir = deployment_dir(deployment_name)?;
    let filename = match file {
        "deployment" => "deployment.toml",
        "build" => "build.toml",
        "monitoring" => "monitoring.toml",
        other => return Err(eyre!("Unknown config file: {other}")),
    };
    let file_path = dir.join(filename);
    let contents = fs::read_to_string(&file_path)?;
    let mut doc: toml_edit::DocumentMut = contents
        .parse()
        .map_err(|e| eyre!("Failed to parse {filename}: {e}"))?;

    let parts: Vec<&str> = path.split('.').collect();
    if parts.is_empty() {
        return Err(eyre!("Empty field path"));
    }

    let (parent_parts, field_name) = parts.split_at(parts.len() - 1);
    let field_name = field_name[0];

    // First pass: figure out how deep we can go through regular tables
    // before hitting an inline table (if any).
    let table_depth = {
        let mut depth = 0usize;
        let mut t = doc.as_table();
        for &part in parent_parts {
            match t.get(part).and_then(|item| item.as_table()) {
                Some(table) => {
                    t = table;
                    depth += 1;
                }
                None => break,
            }
        }
        depth
    };

    if table_depth == parent_parts.len() {
        // All parent parts are regular tables — simple path
        let mut table = doc.as_table_mut();
        for &part in parent_parts {
            table = table
                .entry(part)
                .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
                .as_table_mut()
                .ok_or_else(|| eyre!("Expected '{part}' to be a table"))?;
        }

        // Preserve type: integer fields stay integer, array fields stay array
        let existing = table.get(field_name);
        let is_int = existing.map(|v| v.is_integer()).unwrap_or(false);
        let is_array = existing.map(|v| v.is_array()).unwrap_or(false);

        if is_int {
            let int_val: i64 = value
                .parse()
                .map_err(|_| eyre!("'{field_name}' must be a number, got '{value}'"))?;
            table.insert(field_name, toml_edit::value(int_val));
        } else if is_array {
            // Split comma-separated input into a TOML array of strings
            let mut arr = toml_edit::Array::new();
            for item in value.split(',') {
                let trimmed = item.trim();
                if !trimmed.is_empty() {
                    arr.push(trimmed);
                }
            }
            table.insert(field_name, toml_edit::Item::Value(toml_edit::Value::Array(arr)));
        } else {
            table.insert(field_name, toml_edit::value(value));
        }
    } else {
        // We hit an inline table at `table_depth`.
        // Navigate regular tables up to the inline table's parent,
        // then handle the inline table + remaining path.
        let regular_parts = &parent_parts[..table_depth];
        let inline_key = parent_parts[table_depth];
        let remaining_parts = &parent_parts[table_depth + 1..];

        let mut table = doc.as_table_mut();
        for &part in regular_parts {
            table = table
                .get_mut(part)
                .ok_or_else(|| eyre!("Key '{part}' not found"))?
                .as_table_mut()
                .ok_or_else(|| eyre!("'{part}' is not a table"))?;
        }

        // Get the inline table item
        let item = table
            .get_mut(inline_key)
            .ok_or_else(|| eyre!("Key '{inline_key}' not found"))?;
        let val = item
            .as_value_mut()
            .ok_or_else(|| eyre!("'{inline_key}' is not a value"))?;
        let mut it = val
            .as_inline_table_mut()
            .ok_or_else(|| eyre!("'{inline_key}' is not an inline table"))?;

        // Navigate any remaining inline table nesting
        for &part in remaining_parts {
            let inner = it
                .get_mut(part)
                .ok_or_else(|| eyre!("Key '{part}' not found in inline table"))?;
            it = inner
                .as_inline_table_mut()
                .ok_or_else(|| eyre!("'{part}' is not an inline table"))?;
        }

        // Preserve integer type — reject non-numeric input for integer fields
        let is_int = it.get(field_name).map(|v| v.is_integer()).unwrap_or(false);
        if is_int {
            let int_val: i64 = value
                .parse()
                .map_err(|_| eyre!("'{field_name}' must be a number, got '{value}'"))?;
            it.insert(field_name, toml_edit::Value::from(int_val));
        } else {
            it.insert(field_name, toml_edit::Value::from(value));
        }
    }

    // Validate the modified TOML still deserializes into the expected config type
    // before writing to disk. This catches semantically invalid edits early.
    let new_contents = doc.to_string();
    validate_config_contents(file, &new_contents)
        .map_err(|e| eyre!("Config validation failed after edit: {e}"))?;

    fs::write(&file_path, new_contents)?;
    Ok(())
}

/// Validate that TOML content deserializes into the expected config type.
fn validate_config_contents(file: &str, contents: &str) -> Result<()> {
    match file {
        "deployment" => { toml::from_str::<DeploymentToml>(contents)?; }
        "build" => { toml::from_str::<BuildToml>(contents)?; }
        "monitoring" => { toml::from_str::<MonitoringToml>(contents)?; }
        _ => {}
    }
    Ok(())
}

pub fn migrate_deployment(old_name: &str, new_name: &str) -> Result<()> {
    let deployments = deployments_dir()?;
    let old_dir = deployments.join(old_name);
    let new_dir = deployments.join(new_name);

    if !old_dir.exists() {
        return Err(eyre!("Deployment '{old_name}' not found"));
    }
    if new_dir.exists() {
        return Err(eyre!("Deployment '{new_name}' already exists"));
    }

    // Rename directory
    fs::rename(&old_dir, &new_dir)?;

    // Update deployment.name inside deployment.toml
    let deployment_path = new_dir.join("deployment.toml");
    let contents = fs::read_to_string(&deployment_path)?;
    let mut doc: toml_edit::DocumentMut = contents
        .parse()
        .map_err(|e| eyre!("Failed to parse deployment.toml: {e}"))?;

    if let Some(deployment) = doc.get_mut("deployment") {
        if let Some(table) = deployment.as_table_mut() {
            table.insert("name", toml_edit::value(new_name));
        }
    }
    fs::write(&deployment_path, doc.to_string())?;

    // Update global config default if it pointed to old name
    if let Ok(global) = load_global_config() {
        if global.default_deployment.as_deref() == Some(old_name) {
            write_global_config(&GlobalConfig {
                default_deployment: Some(new_name.to_string()),
            })?;
        }
    }

    Ok(())
}

// ── Hypervisor CRUD ────────────────────────────────────────────────────

/// Update a single field in a shared hypervisor config, preserving formatting.
/// `path` is a dotted key like "credentials.host" or "proxmox.node".
pub fn update_hypervisor_field(name: &str, path: &str, value: &str) -> Result<()> {
    let file_path = hypervisors_dir()?.join(format!("{name}.toml"));
    let contents = fs::read_to_string(&file_path)
        .map_err(|e| eyre!("Hypervisor '{name}' not found: {e}"))?;
    let mut doc: toml_edit::DocumentMut = contents
        .parse()
        .map_err(|e| eyre!("Failed to parse {name}.toml: {e}"))?;

    let parts: Vec<&str> = path.split('.').collect();
    if parts.is_empty() {
        return Err(eyre!("Empty field path"));
    }

    let (parent_parts, field_name) = parts.split_at(parts.len() - 1);
    let field_name = field_name[0];

    let mut table = doc.as_table_mut();
    for &part in parent_parts {
        table = table
            .entry(part)
            .or_insert(toml_edit::Item::Table(toml_edit::Table::new()))
            .as_table_mut()
            .ok_or_else(|| eyre!("Expected '{part}' to be a table"))?;
    }

    // Preserve integer type
    let is_int = table
        .get(field_name)
        .map(|v| v.is_integer())
        .unwrap_or(false);
    if is_int {
        let int_val: i64 = value
            .parse()
            .map_err(|_| eyre!("'{field_name}' must be a number, got '{value}'"))?;
        table.insert(field_name, toml_edit::value(int_val));
    } else {
        table.insert(field_name, toml_edit::value(value));
    }

    // Validate before writing
    let new_contents = doc.to_string();
    toml::from_str::<HypervisorConfig>(&new_contents)
        .map_err(|e| eyre!("Config validation failed after edit: {e}"))?;

    fs::write(&file_path, new_contents)?;
    Ok(())
}

/// Create a new hypervisor config with type-appropriate defaults.
pub fn create_hypervisor(name: &str, htype: HypervisorType) -> Result<()> {
    let dir = hypervisors_dir()?;
    fs::create_dir_all(&dir)?;
    let file_path = dir.join(format!("{name}.toml"));

    if file_path.exists() {
        return Err(eyre!("Hypervisor '{name}' already exists"));
    }

    let config = HypervisorConfig {
        hypervisor: HypervisorMeta {
            name: name.to_string(),
            hypervisor_type: htype.clone(),
        },
        credentials: HypervisorCredentials {
            host: String::new(),
            ssh_user: "root".to_string(),
            ssh_port: None,
        },
        proxmox: match htype {
            HypervisorType::Proxmox => Some(ProxmoxHypervisorConfig {
                node: "PVE".to_string(),
                iso_storage: "local".to_string(),
                disk_storage: "local-lvm".to_string(),
                iso_file: "helios-install-vga.iso".to_string(),
            }),
            HypervisorType::LinuxKvm => None,
        },
    };

    let contents = toml::to_string_pretty(&config)?;
    fs::write(&file_path, contents)?;
    Ok(())
}

/// Delete a hypervisor config file.
/// Caller must check for referencing deployments before calling.
pub fn delete_hypervisor(name: &str) -> Result<()> {
    let file_path = hypervisors_dir()?.join(format!("{name}.toml"));
    if !file_path.exists() {
        return Err(eyre!("Hypervisor '{name}' not found"));
    }
    fs::remove_file(&file_path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_toml_edit_string_field() {
        let toml_str = r#"[network]
gateway = "192.168.1.1"
infra_ip = "192.168.1.50"
"#;
        let mut doc: toml_edit::DocumentMut = toml_str.parse().unwrap();
        let table = doc.get_mut("network").unwrap().as_table_mut().unwrap();
        table.insert("gateway", toml_edit::value("10.0.0.1"));

        let result = doc.to_string();
        assert!(result.contains(r#"gateway = "10.0.0.1""#));
        assert!(result.contains(r#"infra_ip = "192.168.1.50""#));
    }

    #[test]
    fn test_toml_edit_integer_preserved() {
        let toml_str = r#"[omicron.overrides]
cockroachdb_redundancy = 3
vdev_count = 3
"#;
        let mut doc: toml_edit::DocumentMut = toml_str.parse().unwrap();
        let table = doc
            .get_mut("omicron").unwrap().as_table_mut().unwrap()
            .get_mut("overrides").unwrap().as_table_mut().unwrap();

        // Check existing value is integer, write integer back
        assert!(table.get("vdev_count").unwrap().is_integer());
        table.insert("vdev_count", toml_edit::value(5_i64));

        let result = doc.to_string();
        assert!(result.contains("vdev_count = 5"));
        assert!(result.contains("cockroachdb_redundancy = 3"));
    }

    #[test]
    fn test_toml_edit_nested_path() {
        let toml_str = r#"[hypervisor]
ref = "proxmox-lab"

[hypervisor.vm]
vmid = 301
cores = 2
"#;
        let mut doc: toml_edit::DocumentMut = toml_str.parse().unwrap();
        let table = doc
            .get_mut("hypervisor").unwrap().as_table_mut().unwrap()
            .get_mut("vm").unwrap().as_table_mut().unwrap();
        table.insert("cores", toml_edit::value(4_i64));

        let result = doc.to_string();
        assert!(result.contains("cores = 4"));
        assert!(result.contains("vmid = 301"));
    }

    #[test]
    fn test_toml_edit_inline_table() {
        // This is the exact format used in our deployment.toml files
        let toml_str = r#"[network]
gateway = "192.168.2.1"
internal_services_range = { first = "192.168.2.70", last = "192.168.2.79" }
instance_pool_range = { first = "192.168.2.81", last = "192.168.2.90" }
"#;
        let mut doc: toml_edit::DocumentMut = toml_str.parse().unwrap();

        // Navigate: network (Table) → internal_services_range (InlineTable) → first
        let network = doc.get_mut("network").unwrap().as_table_mut().unwrap();
        let range_item = network.get_mut("internal_services_range").unwrap();
        let range = range_item.as_value_mut().unwrap().as_inline_table_mut().unwrap();
        range.insert("first", toml_edit::Value::from("10.0.0.1"));

        let result = doc.to_string();
        assert!(result.contains("10.0.0.1"));
        assert!(result.contains(r#"last = "192.168.2.79""#)); // unchanged
        assert!(result.contains(r#"gateway = "192.168.2.1""#)); // unchanged
    }

    #[test]
    fn test_inline_table_navigation() {
        // Test the two-phase navigation: regular table → inline table → field
        let toml_str = r#"[network]
gateway = "192.168.2.1"
internal_services_range = { first = "192.168.2.40", last = "192.168.2.49" }
instance_pool_range = { first = "192.168.2.51", last = "192.168.2.60" }
"#;
        let mut doc: toml_edit::DocumentMut = toml_str.parse().unwrap();

        // Navigate: network (regular table) → internal_services_range (inline table) → first
        let network = doc.get_mut("network").unwrap().as_table_mut().unwrap();
        let item = network.get_mut("internal_services_range").unwrap();
        let it = item.as_value_mut().unwrap().as_inline_table_mut().unwrap();
        it.insert("first", toml_edit::Value::from("10.0.0.99"));

        let result = doc.to_string();
        assert!(result.contains("10.0.0.99"));
        assert!(result.contains(r#"last = "192.168.2.49""#));
        assert!(result.contains(r#"gateway = "192.168.2.1""#));
        // instance_pool_range unchanged
        assert!(result.contains(r#"first = "192.168.2.51""#));
    }

    #[test]
    fn test_update_hypervisor_field_string() {
        let toml_str = r#"[hypervisor]
name = "test-hyp"
type = "proxmox"

[credentials]
host = "10.0.0.1"
ssh_user = "root"

[proxmox]
node = "PVE"
iso_storage = "local"
disk_storage = "local-lvm"
iso_file = "helios.iso"
"#;
        let mut doc: toml_edit::DocumentMut = toml_str.parse().unwrap();
        let table = doc
            .get_mut("credentials")
            .unwrap()
            .as_table_mut()
            .unwrap();
        table.insert("host", toml_edit::value("192.168.2.5"));

        let result = doc.to_string();
        assert!(result.contains(r#"host = "192.168.2.5""#));
        assert!(result.contains(r#"ssh_user = "root""#)); // unchanged
        assert!(result.contains(r#"node = "PVE""#)); // unchanged

        // Validate roundtrip
        let config: super::HypervisorConfig = toml::from_str(&result).unwrap();
        assert_eq!(config.credentials.host, "192.168.2.5");
    }

    #[test]
    fn test_create_hypervisor_proxmox_defaults() {
        let config = super::HypervisorConfig {
            hypervisor: super::HypervisorMeta {
                name: "test".to_string(),
                hypervisor_type: super::HypervisorType::Proxmox,
            },
            credentials: super::HypervisorCredentials {
                host: String::new(),
                ssh_user: "root".to_string(),
                ssh_port: None,
            },
            proxmox: Some(super::ProxmoxHypervisorConfig {
                node: "PVE".to_string(),
                iso_storage: "local".to_string(),
                disk_storage: "local-lvm".to_string(),
                iso_file: "helios-install-vga.iso".to_string(),
            }),
        };

        let s = toml::to_string_pretty(&config).unwrap();
        let parsed: super::HypervisorConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed.hypervisor.name, "test");
        assert_eq!(
            parsed.hypervisor.hypervisor_type,
            super::HypervisorType::Proxmox
        );
        assert!(parsed.credentials.host.is_empty());
        assert_eq!(parsed.credentials.ssh_user, "root");
        assert!(parsed.proxmox.is_some());
        assert_eq!(parsed.proxmox.unwrap().node, "PVE");
    }
}
