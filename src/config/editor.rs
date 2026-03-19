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

pub fn save_hypervisor(config: &HypervisorConfig) -> Result<()> {
    let dir = hypervisors_dir()?;
    fs::create_dir_all(&dir)?;
    let contents = toml::to_string_pretty(config)?;
    fs::write(dir.join(format!("{}.toml", config.hypervisor.name)), contents)?;
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
            match t.get(part) {
                Some(item) if item.is_table() => {
                    t = item.as_table().unwrap();
                    depth += 1;
                }
                _ => break,
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
                .unwrap();
        }

        // Preserve integer type — reject non-numeric input for integer fields
        let is_int = table.get(field_name).map(|v| v.is_integer()).unwrap_or(false);
        if is_int {
            let int_val: i64 = value
                .parse()
                .map_err(|_| eyre!("'{field_name}' must be a number, got '{value}'"))?;
            table.insert(field_name, toml_edit::value(int_val));
            fs::write(&file_path, doc.to_string())?;
            return Ok(());
        }

        table.insert(field_name, toml_edit::value(value));
    } else {
        // We hit an inline table at `table_depth`.
        // Navigate regular tables up to the inline table's parent,
        // then handle the inline table + remaining path.
        let regular_parts = &parent_parts[..table_depth];
        let inline_key = parent_parts[table_depth];
        let remaining_parts = &parent_parts[table_depth + 1..];

        let mut table = doc.as_table_mut();
        for &part in regular_parts {
            table = table.get_mut(part).unwrap().as_table_mut().unwrap();
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
            fs::write(&file_path, doc.to_string())?;
            return Ok(());
        }

        it.insert(field_name, toml_edit::Value::from(value));
    }

    fs::write(&file_path, doc.to_string())?;
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
}
