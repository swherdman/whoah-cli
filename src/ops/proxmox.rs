//! Proxmox VM management via SSH to the Proxmox host.

use color_eyre::{Result, eyre::eyre};

use crate::config::ProxmoxConfig;
use crate::ssh::RemoteHost;

/// Create a Proxmox VM with the given configuration.
/// Runs `qm create` with all parameters in one command.
pub async fn create_vm(host: &dyn RemoteHost, config: &ProxmoxConfig) -> Result<u32> {
    let vm = &config.vm;
    let vmid = vm.vmid;

    // Build the disk spec: e.g. "local-lvm:256"
    let disk_spec = format!("{}:{}", config.disk_storage, vm.disk_gb);

    // Build the disk key based on bus type
    let disk_key = match vm.disk_bus.as_str() {
        "sata" => "sata0",
        "scsi" => "scsi0",
        "ide" => "ide0",
        _ => "sata0",
    };

    // Build the network spec: e.g. "e1000e,bridge=vmbr0,firewall=1"
    let net_spec = format!("{},bridge={},firewall=1", vm.net_model, vm.net_bridge);

    // Build the ISO spec: e.g. "local:iso/helios-install-vga.iso,media=cdrom"
    let iso_spec = format!("{}:iso/{},media=cdrom", config.iso_storage, config.iso_file);

    let cmd = format!(
        "qm create {vmid} \
         --name {} \
         --ostype {} \
         --memory {} \
         --cores {} \
         --sockets {} \
         --cpu {} \
         --{disk_key} {disk_spec} \
         --net0 {net_spec} \
         --serial0 socket \
         --vga serial0 \
         --ide2 {iso_spec} \
         --boot order=ide2",
        vm.name, vm.os_type, vm.memory_mb, vm.cores, vm.sockets, vm.cpu_type,
    );

    let output = host.execute(&cmd).await?;
    if output.exit_code != 0 {
        return Err(eyre!(
            "qm create failed (exit {}): {}",
            output.exit_code,
            output.stderr.trim()
        ));
    }

    Ok(vmid)
}

/// Start a Proxmox VM.
pub async fn start_vm(host: &dyn RemoteHost, vmid: u32) -> Result<()> {
    let cmd = format!("qm start {vmid}");
    let output = host.execute(&cmd).await?;
    if output.exit_code != 0 {
        return Err(eyre!(
            "qm start failed (exit {}): {}",
            output.exit_code,
            output.stderr.trim()
        ));
    }
    Ok(())
}

/// Check if a VM exists on the Proxmox host.
pub async fn vm_exists(host: &dyn RemoteHost, vmid: u32) -> Result<bool> {
    let cmd = format!("qm status {vmid}");
    let output = host.execute(&cmd).await?;
    Ok(output.exit_code == 0)
}

/// Get the status of a VM (e.g., "running", "stopped").
pub async fn vm_status(host: &dyn RemoteHost, vmid: u32) -> Result<String> {
    let cmd = format!("qm status {vmid}");
    let output = host.execute(&cmd).await?;
    if output.exit_code != 0 {
        return Err(eyre!(
            "qm status failed (exit {}): {}",
            output.exit_code,
            output.stderr.trim()
        ));
    }
    // Output is like "status: running"
    let status = output
        .stdout
        .trim()
        .strip_prefix("status: ")
        .unwrap_or(output.stdout.trim())
        .to_string();
    Ok(status)
}

/// Wait for a VM to reach "running" state, polling at intervals.
pub async fn wait_for_running(host: &dyn RemoteHost, vmid: u32) -> Result<()> {
    for _ in 0..30 {
        match vm_status(host, vmid).await {
            Ok(status) if status == "running" => return Ok(()),
            Ok(_) => {}
            Err(_) => {}
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    Err(eyre!("VM {vmid} did not reach running state within 60s"))
}
