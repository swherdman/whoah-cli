use std::fs;
use std::path::PathBuf;

use color_eyre::{eyre::eyre, Result};

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
        return Err(eyre!("Deployment '{}' not found at {}", name, dir.display()));
    }

    let deployment_path = dir.join("deployment.toml");
    let build_path = dir.join("build.toml");
    let monitoring_path = dir.join("monitoring.toml");

    let deployment: DeploymentToml = {
        let contents = fs::read_to_string(&deployment_path).map_err(|e| {
            eyre!("Failed to read {}: {}", deployment_path.display(), e)
        })?;
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

pub fn list_deployments() -> Result<Vec<String>> {
    let dir = deployments_dir()?;
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut names = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                // Only list dirs that have a deployment.toml
                if entry.path().join("deployment.toml").exists() {
                    names.push(name.to_string());
                }
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
        1 => Ok(deployments.into_iter().next().unwrap()),
        _ => Err(eyre!(
            "Multiple deployments found: {}. Use --deployment <name> to select one.",
            deployments.join(", ")
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_explicit_wins() {
        let result = resolve_deployment(Some("my-lab"));
        assert_eq!(result.unwrap(), "my-lab");
    }
}
