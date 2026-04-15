use color_eyre::Result;

use crate::config;
use crate::ops::status::{format_status, gather_status};
use crate::ssh::session::SshHost;

pub async fn run(deployment: Option<&str>) -> Result<()> {
    let deployment_name = config::resolve_deployment(deployment)?;
    let cfg = config::load_deployment(&deployment_name)?;

    // Use the first host (Phase 1: single host)
    let host_config = cfg.deployment.hosts.values().next().ok_or_else(|| {
        color_eyre::eyre::eyre!("No hosts configured in deployment '{deployment_name}'")
    })?;

    eprintln!(
        "Connecting to {}@{}...",
        host_config.ssh_user, host_config.address
    );

    let host = SshHost::connect(host_config).await?;
    let status = gather_status(&host, &cfg).await?;
    let output = format_status(&status, &deployment_name);
    print!("{output}");

    // Explicit async cleanup — prevents orphaned SSH master processes
    host.close().await?;

    Ok(())
}
