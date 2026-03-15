use std::fs;

use color_eyre::Result;

use super::loader::{deployment_dir, whoah_dir};
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
