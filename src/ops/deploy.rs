//! Deploy pipeline executor.
//!
//! Orchestrates the build & deploy pipeline by running each step in sequence
//! and sending progress events back to the TUI.

use std::path::PathBuf;
use std::time::Duration;

use color_eyre::{eyre::eyre, Result};
use tokio::sync::mpsc;

use crate::config::{DeploymentConfig, HostConfig, ProxmoxConfig};
use crate::event::BuildEvent;
use crate::ops::proxmox;
use crate::ops::serial::SerialConsole;
use crate::ssh::direct::DirectSsh;
use crate::ssh::session::SshHost;
use crate::ssh::RemoteHost;

/// Run the full deploy pipeline, sending progress events through `tx`.
pub async fn run_deploy(
    config: DeploymentConfig,
    tx: mpsc::UnboundedSender<BuildEvent>,
) -> Result<()> {
    let proxmox_config = config
        .deployment
        .proxmox
        .as_ref()
        .ok_or_else(|| eyre!("No [proxmox] config in deployment.toml"))?;

    // Connect to Proxmox host
    let pve_host_config = HostConfig {
        address: proxmox_config.host.clone(),
        ssh_user: proxmox_config.ssh_user.clone(),
        role: crate::config::HostRole::Combined,
    };
    let pve = SshHost::connect(&pve_host_config).await?;

    // Phase 1: Provision VM
    let helios_ip = run_provision(&pve, proxmox_config, &tx).await?;

    // Phase 2: Configure Access
    let ssh_user = run_configure_access(&helios_ip, &tx).await?;

    // Phase 3: Build & Deploy
    run_setup_pkg_cache(&helios_ip, &ssh_user, &tx).await?;
    run_os_setup(&helios_ip, &ssh_user, &config, &tx).await?;

    // Phase 3 continued: Omicron build & deploy
    run_omicron_build(&helios_ip, &ssh_user, &config, &tx).await?;

    // Phase 4: future increments (patches)

    // Cleanup
    let _ = pve.close().await;
    Ok(())
}

async fn run_provision(
    pve: &SshHost,
    config: &ProxmoxConfig,
    tx: &mpsc::UnboundedSender<BuildEvent>,
) -> Result<String> {
    let vmid = config.vm.vmid;

    // Step: Create Proxmox VM
    send(tx, BuildEvent::StepStarted("prov-create".into()));
    send(
        tx,
        BuildEvent::StepDetail(
            "prov-create".into(),
            format!("Creating VM {} (VMID {})...", config.vm.name, vmid),
        ),
    );

    if proxmox::vm_exists(pve, vmid).await? {
        send(
            tx,
            BuildEvent::StepFailed(
                "prov-create".into(),
                format!("VM {vmid} already exists. Delete it first or choose a different VMID."),
            ),
        );
        return Err(eyre!("VM {vmid} already exists"));
    }

    match proxmox::create_vm(pve, config).await {
        Ok(_) => send(tx, BuildEvent::StepCompleted("prov-create".into())),
        Err(e) => {
            send(tx, BuildEvent::StepFailed("prov-create".into(), e.to_string()));
            return Err(e);
        }
    }

    // Step: Boot Helios ISO
    send(tx, BuildEvent::StepStarted("prov-boot".into()));
    send(
        tx,
        BuildEvent::StepDetail("prov-boot".into(), format!("Starting VM {vmid}...")),
    );

    if let Err(e) = proxmox::start_vm(pve, vmid).await {
        send(tx, BuildEvent::StepFailed("prov-boot".into(), e.to_string()));
        return Err(e);
    }

    send(
        tx,
        BuildEvent::StepDetail("prov-boot".into(), "Waiting for VM to reach running state...".into()),
    );

    if let Err(e) = proxmox::wait_for_running(pve, vmid).await {
        send(tx, BuildEvent::StepFailed("prov-boot".into(), e.to_string()));
        return Err(e);
    }

    send(tx, BuildEvent::StepCompleted("prov-boot".into()));

    // Step: Install Helios via serial console
    send(tx, BuildEvent::StepStarted("prov-install".into()));
    send(
        tx,
        BuildEvent::StepDetail("prov-install".into(), "Connecting to serial console...".into()),
    );

    let log_dir = crate::config::loader::whoah_dir()
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("logs");
    let log_path = log_dir.join("serial-install.log");

    let mut console = SerialConsole::connect_with_log(
        &config.host,
        &config.ssh_user,
        vmid,
        Some(log_path),
    )
    .await
    .map_err(|e| {
        send(tx, BuildEvent::StepFailed("prov-install".into(), e.to_string()));
        e
    })?;

    send(
        tx,
        BuildEvent::StepDetail("prov-install".into(), "Waiting for Helios ISO to boot...".into()),
    );

    // Wait for shell prompt — the ISO takes 30-60s to boot.
    // Periodically send CR to nudge the console once the shell is ready.
    let tx_ref = tx;
    console
        .wait_for_prompt(
            Duration::from_secs(180),
            Duration::from_secs(5),
            |line| { send(tx_ref, BuildEvent::StepDetail("prov-install".into(), line.to_string())); },
        )
        .await
        .map_err(|e| {
            send(tx_ref, BuildEvent::StepFailed("prov-install".into(), format!("No shell prompt: {e}")));
            e
        })?;

    // Run diskinfo to find available disks
    send(
        tx_ref,
        BuildEvent::StepDetail("prov-install".into(), "Running diskinfo...".into()),
    );
    console.send("diskinfo").await?;

    // Collect diskinfo output — wait for the prompt to return
    let mut diskinfo_lines: Vec<String> = Vec::new();
    console
        .wait_for(
            Duration::from_secs(15),
            |line| {
                diskinfo_lines.push(line.to_string());
                send(tx_ref, BuildEvent::StepDetail("prov-install".into(), line.to_string()));
            },
            |line| line.trim().ends_with('#'),
        )
        .await
        .map_err(|e| {
            send(tx_ref, BuildEvent::StepFailed("prov-install".into(), format!("diskinfo failed: {e}")));
            e
        })?;

    // Parse disk names from diskinfo output
    // Lines look like: "SATA    c2t0d0                  QEMU     HARDDISK          256.00 GiB"
    let disks: Vec<String> = diskinfo_lines
        .iter()
        .filter(|l| l.starts_with("SATA") || l.starts_with("NVMe") || l.starts_with("USB"))
        .filter_map(|l| l.split_whitespace().nth(1).map(String::from))
        .collect();

    if disks.is_empty() {
        let msg = "No disks found in diskinfo output".to_string();
        send(tx_ref, BuildEvent::StepFailed("prov-install".into(), msg.clone()));
        return Err(eyre!("{msg}"));
    }

    // Use the first disk (Proxmox VMs typically have one)
    let disk = &disks[0];
    let hostname = &config.vm.name;

    send(
        tx_ref,
        BuildEvent::StepDetail(
            "prov-install".into(),
            format!("Installing Helios to {disk} as '{hostname}'..."),
        ),
    );

    console.send(&format!("install-helios {hostname} {disk}")).await?;

    // Wait for install to complete — this can take a few minutes
    // install-helios prints progress and eventually returns to a prompt
    console
        .wait_for(
            Duration::from_secs(600),
            |line| { send(tx_ref, BuildEvent::StepDetail("prov-install".into(), line.to_string())); },
            |line| line.trim().ends_with('#'),
        )
        .await
        .map_err(|e| {
            send(tx_ref, BuildEvent::StepFailed("prov-install".into(), format!("install-helios failed: {e}")));
            e
        })?;

    // Eject ISO and change boot order to disk before rebooting
    send(
        tx_ref,
        BuildEvent::StepDetail("prov-install".into(), "Ejecting ISO and setting boot to disk...".into()),
    );
    let eject_cmd = format!(
        "qm set {} --ide2 none,media=cdrom --boot order=sata0",
        config.vm.vmid
    );
    let eject_result = pve.execute(&eject_cmd).await?;
    if eject_result.exit_code != 0 {
        send(
            tx_ref,
            BuildEvent::StepFailed(
                "prov-install".into(),
                format!("Failed to eject ISO: {}", eject_result.stderr.trim()),
            ),
        );
        return Err(eyre!("Failed to eject ISO"));
    }

    // Drop the console before stopping the VM
    drop(console);

    // Stop and start the VM from the Proxmox host side so QEMU picks up
    // the new boot order. A guest-initiated reboot is a warm reboot inside
    // the same QEMU process and won't apply config changes.
    send(
        tx_ref,
        BuildEvent::StepDetail("prov-install".into(), "Stopping VM to apply boot config...".into()),
    );
    let stop_result = pve.execute(&format!("qm stop {vmid}")).await?;
    if stop_result.exit_code != 0 {
        // VM might have already shut down from the install
        tracing::warn!("qm stop returned {}: {}", stop_result.exit_code, stop_result.stderr.trim());
    }

    // Wait for VM to fully stop
    for _ in 0..30 {
        match proxmox::vm_status(pve, vmid).await {
            Ok(status) if status == "stopped" => break,
            _ => tokio::time::sleep(Duration::from_secs(2)).await,
        }
    }

    send(
        tx_ref,
        BuildEvent::StepDetail("prov-install".into(), "Starting VM from disk...".into()),
    );
    proxmox::start_vm(pve, vmid).await.map_err(|e| {
        send(tx_ref, BuildEvent::StepFailed("prov-install".into(), format!("Failed to start VM: {e}")));
        e
    })?;

    send(tx_ref, BuildEvent::StepCompleted("prov-install".into()));

    // Step: Configure networking
    send(tx_ref, BuildEvent::StepStarted("prov-network".into()));
    send(
        tx_ref,
        BuildEvent::StepDetail("prov-network".into(), "Waiting for VM to reboot...".into()),
    );

    // Wait for the VM to come back up
    proxmox::wait_for_running(pve, vmid).await.map_err(|e| {
        send(tx_ref, BuildEvent::StepFailed("prov-network".into(), format!("VM didn't restart: {e}")));
        e
    })?;

    // Give the serial socket a moment to be recreated by QEMU
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Open a new serial console connection
    send(
        tx_ref,
        BuildEvent::StepDetail("prov-network".into(), "Reconnecting to serial console...".into()),
    );

    let log_path_net = log_dir.join("serial-network.log");
    let mut console = SerialConsole::connect_with_log(
        &config.host,
        &config.ssh_user,
        vmid,
        Some(log_path_net),
    )
    .await
    .map_err(|e| {
        send(tx_ref, BuildEvent::StepFailed("prov-network".into(), format!("Serial reconnect failed: {e}")));
        e
    })?;

    // Wait for login prompt or shell prompt (boot from disk takes 30-60s)
    send(
        tx_ref,
        BuildEvent::StepDetail("prov-network".into(), "Waiting for Helios to boot from disk...".into()),
    );

    let post_reboot = console
        .wait_for(
            Duration::from_secs(300),
            |line| { send(tx_ref, BuildEvent::StepDetail("prov-network".into(), line.to_string())); },
            |line| line.contains("login:") || line.trim().ends_with('#'),
        )
        .await
        .map_err(|e| {
            send(tx_ref, BuildEvent::StepFailed("prov-network".into(), format!("Boot timeout: {e}")));
            e
        })?;

    // Handle login: root has no password, send username then empty password
    if post_reboot.contains("login:") {
        console.send("root").await?;

        // Wait for Password: prompt or direct shell
        let login_response = console
            .wait_for(
                Duration::from_secs(15),
                |line| { send(tx_ref, BuildEvent::StepDetail("prov-network".into(), line.to_string())); },
                |line| line.contains("Password:") || line.trim().ends_with('#'),
            )
            .await?;

        if login_response.contains("Password:") {
            // Send empty password (root has no password)
            console.send("").await?;
            console
                .wait_for(
                    Duration::from_secs(15),
                    |line| { send(tx_ref, BuildEvent::StepDetail("prov-network".into(), line.to_string())); },
                    |line| line.trim().ends_with('#'),
                )
                .await
                .map_err(|e| {
                    send(tx_ref, BuildEvent::StepFailed("prov-network".into(), format!("Login failed: {e}")));
                    e
                })?;
        }
    }

    // Configure network interface
    send(
        tx_ref,
        BuildEvent::StepDetail("prov-network".into(), "Configuring network interface...".into()),
    );

    console.send("ipadm create-if e1000g0").await?;
    console
        .wait_for(Duration::from_secs(10), |_| {}, |line| line.trim().ends_with('#'))
        .await?;

    console
        .send(&format!("ipadm create-addr -T dhcp -h {hostname} e1000g0/dhcp"))
        .await?;
    console
        .wait_for(Duration::from_secs(10), |_| {}, |line| line.trim().ends_with('#'))
        .await?;

    console.send("svcadm restart network/service").await?;
    console
        .wait_for(Duration::from_secs(10), |_| {}, |line| line.trim().ends_with('#'))
        .await?;

    // Wait a moment for DHCP, then get the IP
    send(
        tx_ref,
        BuildEvent::StepDetail("prov-network".into(), "Waiting for DHCP address...".into()),
    );

    // Poll ipadm show-addr until we see an IP on e1000g0
    let mut ip_address = String::new();
    for _ in 0..15 {
        console.send("ipadm show-addr -o ADDR e1000g0/dhcp").await?;
        let mut addr_lines = Vec::new();
        console
            .wait_for(
                Duration::from_secs(5),
                |line| addr_lines.push(line.to_string()),
                |line| line.trim().ends_with('#'),
            )
            .await?;

        // Look for an IP address in the output
        for line in &addr_lines {
            let trimmed = line.trim();
            // Skip header lines and empty lines
            if trimmed.is_empty()
                || trimmed.starts_with("ADDR")
                || trimmed.starts_with("ipadm")
                || trimmed.ends_with('#')
            {
                continue;
            }
            // Extract IP (format: "192.168.2.x/24" or just "192.168.2.x")
            if let Some(ip) = trimmed.split('/').next() {
                if ip.contains('.') {
                    ip_address = ip.to_string();
                    break;
                }
            }
        }

        if !ip_address.is_empty() {
            break;
        }

        tokio::time::sleep(Duration::from_secs(3)).await;
    }

    if ip_address.is_empty() {
        let msg = "Could not obtain DHCP address after 45s".to_string();
        send(tx_ref, BuildEvent::StepFailed("prov-network".into(), msg.clone()));
        return Err(eyre!("{msg}"));
    }

    send(
        tx_ref,
        BuildEvent::StepDetail(
            "prov-network".into(),
            format!("Network configured — IP: {ip_address}"),
        ),
    );
    send(tx_ref, BuildEvent::StepCompleted("prov-network".into()));

    // Step: Netcat user setup
    send(tx_ref, BuildEvent::StepStarted("prov-netcat".into()));
    send(
        tx_ref,
        BuildEvent::StepDetail(
            "prov-netcat".into(),
            "Starting netcat listener on port 1701...".into(),
        ),
    );

    // Start nc listener on the Helios host via serial console
    // This blocks until data is received, so we start it and move on to Phase 2
    // which will send the userdata script via nc from the workstation.
    console.send("nc -l 1701 </dev/null | bash -x &").await?;
    console
        .wait_for(Duration::from_secs(5), |_| {}, |line| line.trim().ends_with('#'))
        .await?;

    send(
        tx_ref,
        BuildEvent::StepDetail(
            "prov-netcat".into(),
            format!("Netcat listener ready on {ip_address}:1701"),
        ),
    );
    send(tx_ref, BuildEvent::StepCompleted("prov-netcat".into()));

    Ok(ip_address)
}

/// Phase 2: Configure SSH access to the Helios host.
/// Generates a userdata script and sends it via netcat, then verifies SSH.
async fn run_configure_access(
    helios_ip: &str,
    tx: &mpsc::UnboundedSender<BuildEvent>,
) -> Result<String> {
    // Step: Send SSH keys
    send(tx, BuildEvent::StepStarted("access-keys".into()));
    send(
        tx,
        BuildEvent::StepDetail("access-keys".into(), "Generating userdata script...".into()),
    );

    let script = generate_userdata_script().await?;
    let username = get_local_username()?;

    send(
        tx,
        BuildEvent::StepDetail(
            "access-keys".into(),
            format!("Sending userdata to {helios_ip}:1701 (user: {username})..."),
        ),
    );

    // Send the script via netcat to the Helios host
    let mut nc = tokio::process::Command::new("nc")
        .args([helios_ip, "1701"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| eyre!("Failed to spawn nc: {e}"))?;

    if let Some(mut stdin) = nc.stdin.take() {
        use tokio::io::AsyncWriteExt;
        stdin.write_all(script.as_bytes()).await
            .map_err(|e| eyre!("Failed to write to nc: {e}"))?;
        // Drop stdin to close the pipe and signal EOF to nc
    }

    let nc_result = nc.wait().await
        .map_err(|e| eyre!("nc failed: {e}"))?;

    if !nc_result.success() {
        let msg = format!("nc exited with code {:?}", nc_result.code());
        send(tx, BuildEvent::StepFailed("access-keys".into(), msg.clone()));
        return Err(eyre!("{msg}"));
    }

    // Give the remote script a moment to execute
    tokio::time::sleep(Duration::from_secs(5)).await;

    send(tx, BuildEvent::StepCompleted("access-keys".into()));

    // Step: Verify SSH connectivity
    send(tx, BuildEvent::StepStarted("access-verify".into()));
    send(
        tx,
        BuildEvent::StepDetail(
            "access-verify".into(),
            format!("Connecting via SSH as {username}@{helios_ip}..."),
        ),
    );

    let helios_host_config = HostConfig {
        address: helios_ip.to_string(),
        ssh_user: username.clone(),
        role: crate::config::HostRole::Combined,
    };

    let helios = SshHost::connect(&helios_host_config).await.map_err(|e| {
        send(
            tx,
            BuildEvent::StepFailed("access-verify".into(), format!("SSH failed: {e}")),
        );
        e
    })?;

    // Run a quick command to verify
    let output = helios.execute("hostname").await.map_err(|e| {
        send(
            tx,
            BuildEvent::StepFailed("access-verify".into(), format!("Command failed: {e}")),
        );
        e
    })?;

    send(
        tx,
        BuildEvent::StepDetail(
            "access-verify".into(),
            format!("SSH connected — hostname: {}", output.stdout.trim()),
        ),
    );

    let _ = helios.close().await;
    send(tx, BuildEvent::StepCompleted("access-verify".into()));

    Ok(username)
}

/// Generate the userdata setup script (equivalent of gen_userdata.sh).
/// This script creates the local user account on Helios with SSH key access.
async fn generate_userdata_script() -> Result<String> {
    let username = get_local_username()?;
    let uid = get_local_uid()?;
    let gecos = get_local_gecos()?;

    // Read SSH authorized_keys
    let home = std::env::var("HOME").map_err(|_| eyre!("HOME not set"))?;
    let auth_keys_path = std::path::Path::new(&home).join(".ssh").join("authorized_keys");

    // If no authorized_keys, try to build one from public key files
    let auth_keys = if auth_keys_path.exists() {
        tokio::fs::read_to_string(&auth_keys_path)
            .await
            .map_err(|e| eyre!("Failed to read {}: {e}", auth_keys_path.display()))?
    } else {
        // Look for individual .pub files
        let ssh_dir = std::path::Path::new(&home).join(".ssh");
        let mut keys = String::new();
        if let Ok(mut entries) = tokio::fs::read_dir(&ssh_dir).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("pub") {
                    if let Ok(content) = tokio::fs::read_to_string(&path).await {
                        keys.push_str(&content);
                        if !content.ends_with('\n') {
                            keys.push('\n');
                        }
                    }
                }
            }
        }
        if keys.is_empty() {
            return Err(eyre!("No SSH keys found in ~/.ssh/"));
        }
        keys
    };

    let script = format!(
        r#"#!/bin/bash
set -o errexit
set -o pipefail
set -o xtrace
echo 'Just a moment...' >/dev/msglog
/sbin/zfs create 'rpool/home/{username}'
/usr/sbin/useradd -u '{uid}' -g staff -c '{gecos}' -d '/home/{username}' \
    -P 'Primary Administrator' -s /bin/bash '{username}'
/bin/passwd -N '{username}'
/bin/mkdir -p '/home/{username}/.ssh'
cat > '/home/{username}/.ssh/authorized_keys' <<'EOSSH'
{auth_keys}EOSSH
/bin/chmod 600 '/home/{username}/.ssh/authorized_keys'
/bin/chown -R '{username}:staff' '/home/{username}'
/bin/chmod 0700 '/home/{username}'
/bin/sed -i \
    -e '/^PATH=/s#$#:/opt/ooce/bin:/opt/ooce/sbin#' \
    /etc/default/login
/bin/ntpdig -S 0.pool.ntp.org || true
echo 'ok go' >/dev/msglog
"#
    );

    Ok(script)
}

fn get_local_username() -> Result<String> {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .map_err(|_| eyre!("Could not determine local username"))
}

fn get_local_uid() -> Result<u32> {
    Ok(unsafe { libc::getuid() })
}

fn get_local_gecos() -> Result<String> {
    let username = get_local_username()?;
    // Try getent passwd
    let output = std::process::Command::new("getent")
        .args(["passwd", &username])
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let line = String::from_utf8_lossy(&out.stdout);
            let gecos = line.split(':').nth(4).unwrap_or(&username).to_string();
            Ok(gecos)
        }
        _ => Ok(username), // Fallback to username
    }
}

/// Phase 3 (partial): OS setup — update, packages, Rust, swap.
async fn run_os_setup(
    helios_ip: &str,
    ssh_user: &str,
    config: &DeploymentConfig,
    tx: &mpsc::UnboundedSender<BuildEvent>,
) -> Result<()> {
    let helios_config = HostConfig {
        address: helios_ip.to_string(),
        ssh_user: ssh_user.to_string(),
        role: crate::config::HostRole::Combined,
    };

    let log_dir = crate::config::loader::whoah_dir()
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("logs");
    let log_path = log_dir.join("os-setup.log");

    let helios = DirectSsh::connect(&helios_config).await?;
    helios.set_label("Build/pkg-update");

    // --- Step: OS update + reboot ---
    send(tx, BuildEvent::StepStarted("build-pkg-update".into()));

    {
        let mut ssh = crate::ops::ssh_log::LoggedSsh::new(
            &helios, log_path.clone(), tx, "build-pkg-update",
        ).await?;

        ssh.detail("Running pkg update...").await;
        // pkg update: exit 0 = updated (reboot needed), exit 4 = nothing to do
        let exit_code = ssh.run_streaming("pfexec pkg update -v 2>&1; echo \"PKG_EXIT=$?\"").await?;

        // Parse the real exit code from our echo (ssh exit code may differ)
        // For simplicity, check if the output indicated no updates
        let check = ssh.run("beadm list -H | wc -l").await?;
        let be_count: usize = check.stdout.trim().parse().unwrap_or(1);

        if be_count <= 1 {
            // Only one BE — no update happened
            ssh.detail("Already up to date — skipping reboot").await;
            send(tx, BuildEvent::StepCompleted("build-pkg-update".into()));
        } else {
            ssh.detail("Rebooting for OS update...").await;

            // Fire and forget — reboot kills the connection, don't wait for it.
            // Use a short timeout so we don't block for 120s.
            let reboot_result = tokio::time::timeout(
                Duration::from_secs(5),
                ssh.run("pfexec reboot"),
            ).await;
            // Expected: timeout or connection error. Both are fine.
            tracing::info!("Reboot command result: {:?}", reboot_result.is_ok());

            send(tx, BuildEvent::StepCompleted("build-pkg-update".into()));

            // Drop the SSH connection immediately
            drop(ssh);
            let _ = helios.close().await;

            // Wait for SSH to go down (confirm reboot started)
            send(tx, BuildEvent::StepDetail(
                "build-del-be".into(),
                "Waiting for host to reboot...".into(),
            ));
            tokio::time::sleep(Duration::from_secs(10)).await;

            // Wait for SSH to come back
            wait_for_ssh(helios_ip, ssh_user, Duration::from_secs(300)).await?;

            return continue_os_setup_after_reboot(
                helios_ip, ssh_user, config, tx, &log_path,
            ).await;
        }
    }

    // If no reboot needed, continue inline
    let _ = helios.close().await;
    continue_os_setup_after_reboot(
        helios_ip, ssh_user, config, tx, &log_path,
    ).await
}

async fn continue_os_setup_after_reboot(
    helios_ip: &str,
    ssh_user: &str,
    _config: &DeploymentConfig,
    tx: &mpsc::UnboundedSender<BuildEvent>,
    log_path: &PathBuf,
) -> Result<()> {
    // Use DirectSsh for build pipeline commands — the openssh crate's mux has
    // a bug where it refuses new channels. DirectSsh uses the system ssh binary
    // with OS-level ControlMaster which is proven reliable.
    // See docs/BUG-ssh-mux-channel-refusal.md
    let helios_config = HostConfig {
        address: helios_ip.to_string(),
        ssh_user: ssh_user.to_string(),
        role: crate::config::HostRole::Combined,
    };

    let helios = DirectSsh::connect(&helios_config).await?;
    helios.set_label("Build/OS-setup");
    let mut ssh = crate::ops::ssh_log::LoggedSsh::new(
        &helios, log_path.clone(), tx, "build-del-be",
    ).await?;

    // Re-set pkg publisher and HTTPS proxy (new BE has original publisher)
    ssh.detail("Re-setting caches after reboot...").await;
    let cache_info = crate::ops::pkg_cache::ensure_caches().await?;
    let _ = ssh.run(&format!(
        "pfexec pkg set-publisher -O {} helios-dev",
        cache_info.publisher_url
    )).await;

    // Install CA cert and set proxy for HTTPS downloads
    let ca_path = crate::ops::pkg_cache::install_ca_cert(&helios, &cache_info.lan_ip).await
        .unwrap_or_else(|_| "/etc/certs/CA/whoah-cache-ca.pem".to_string());
    ssh.set_proxy(&cache_info.https_proxy_url, &ca_path);

    // --- Step: Delete old boot environments ---
    send(tx, BuildEvent::StepStarted("build-del-be".into()));
    ssh.detail("Listing boot environments...").await;
    let be_output = ssh.run("beadm list -H").await?;

    for line in be_output.stdout.lines() {
        let fields: Vec<&str> = line.split(';').collect();
        if fields.len() >= 3 {
            let be_name = fields[0];
            let active = fields[2];
            if !active.contains('N') && !active.contains('R') {
                ssh.detail(&format!("Deleting old BE: {be_name}")).await;
                let _ = ssh.run(&format!("pfexec beadm destroy -Ff {be_name}")).await;
            }
        }
    }
    send(tx, BuildEvent::StepCompleted("build-del-be".into()));

    // --- Step: Install packages ---
    send(tx, BuildEvent::StepStarted("build-packages".into()));
    ssh.set_step("build-packages");
    ssh.detail("Installing required packages...").await;

    let pkg_result = ssh.run_streaming(
        "pfexec pkg install -v \
         /developer/build-essential \
         /developer/illumos-tools \
         /developer/pkg-config \
         /library/libxmlsec1 \
         /system/zones/brand/omicron1/tools \
         2>&1"
    ).await?;

    if pkg_result != 0 && pkg_result != 4 {
        ssh.fail(&format!("pkg install failed with exit code {pkg_result}")).await;
        return Err(eyre!("pkg install failed"));
    }
    send(tx, BuildEvent::StepCompleted("build-packages".into()));

    // --- Step: Install Rust ---
    send(tx, BuildEvent::StepStarted("build-rust".into()));
    ssh.set_step("build-rust");

    let toolchain = "1.91.1";
    ssh.detail(&format!("Installing Rust toolchain {toolchain}...")).await;

    let rust_check = ssh.run("bash -c '. ~/.cargo/env 2>/dev/null; rustc -V 2>/dev/null'").await?;
    if rust_check.exit_code == 0 && rust_check.stdout.contains(toolchain) {
        ssh.detail(&format!("Rust {toolchain} already installed")).await;
    } else {
        ssh.run_streaming_check_with_proxy(
            &format!(
                "bash -c 'curl --proto =https --tlsv1.2 -sSf https://sh.rustup.rs | \
                 bash -s -- -y --default-toolchain {toolchain}' 2>&1"
            )
        ).await.map_err(|e| {
            let _ = tx.send(BuildEvent::StepFailed("build-rust".into(), e.to_string()));
            e
        })?;

        let verify = ssh.run("bash -c '. ~/.cargo/env && rustc -V'").await?;
        ssh.detail(&format!("Installed: {}", verify.stdout.trim())).await;
    }
    send(tx, BuildEvent::StepCompleted("build-rust".into()));

    // --- Step: Configure swap ---
    send(tx, BuildEvent::StepStarted("build-swap".into()));
    ssh.set_step("build-swap");
    ssh.detail("Checking swap...").await;

    let swap_check = ssh.run("swap -l").await?;
    if swap_check.stdout.contains("swapfile") && swap_check.stdout.lines().count() > 1 {
        ssh.detail("Swap already configured").await;
    } else {
        ssh.detail("Creating 8GB swap...").await;
        let zfs_check = ssh.run("zfs list rpool/swap 2>/dev/null").await?;
        if zfs_check.exit_code != 0 {
            ssh.run_check("pfexec zfs create -V 8G rpool/swap").await?;
        }
        let vfstab = ssh.run("grep rpool/swap /etc/vfstab").await?;
        if vfstab.exit_code != 0 {
            ssh.run_check(
                "echo '/dev/zvol/dsk/rpool/swap - - swap - no -' | pfexec tee -a /etc/vfstab"
            ).await?;
        }
        ssh.run_check("pfexec /usr/sbin/swap -a /dev/zvol/dsk/rpool/swap").await?;
        ssh.detail("Swap configured").await;
    }
    send(tx, BuildEvent::StepCompleted("build-swap".into()));

    let _ = helios.close().await;
    Ok(())
}

/// Phase 3 continued: Clone, build, and deploy Omicron.
async fn run_omicron_build(
    helios_ip: &str,
    ssh_user: &str,
    config: &DeploymentConfig,
    tx: &mpsc::UnboundedSender<BuildEvent>,
) -> Result<()> {
    let helios_config = HostConfig {
        address: helios_ip.to_string(),
        ssh_user: ssh_user.to_string(),
        role: crate::config::HostRole::Combined,
    };

    let log_dir = crate::config::loader::whoah_dir()
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
        .join("logs");
    let log_path = log_dir.join("omicron-build.log");

    let helios = DirectSsh::connect(&helios_config).await?;
    helios.set_label("Build/Omicron");
    let mut ssh = crate::ops::ssh_log::LoggedSsh::new(
        &helios, log_path.clone(), tx, "build-clone",
    ).await?;

    // Re-set proxy for HTTPS downloads
    let cache_info = crate::ops::pkg_cache::ensure_caches().await?;
    let ca_path = crate::ops::pkg_cache::install_ca_cert(&helios, &cache_info.lan_ip).await
        .unwrap_or_else(|_| "/etc/certs/CA/whoah-cache-ca.pem".to_string());
    ssh.set_proxy(&cache_info.https_proxy_url, &ca_path);

    let repo_path = &config.build.omicron.repo_path;
    let overrides = &config.build.omicron.overrides;
    let network = &config.deployment.network;

    // --- Step: Clone omicron ---
    send(tx, BuildEvent::StepStarted("build-clone".into()));
    ssh.detail("Cloning omicron repository...").await;

    let check = ssh.run(&format!("test -d {repo_path}/.git && echo exists")).await?;
    if check.stdout.trim() == "exists" {
        ssh.detail("Omicron repo already exists, pulling latest...").await;
        let _ = ssh.run(&format!("cd {repo_path} && git fetch")).await;
    } else {
        // Don't proxy git clone — git doesn't trust our self-signed CA
        ssh.run_streaming_check(&format!(
            "git clone https://github.com/oxidecomputer/omicron.git {repo_path} 2>&1"
        )).await?;
    }

    send(tx, BuildEvent::StepCompleted("build-clone".into()));

    // --- Step: Configure network IPs (early — before packaging bakes configs) ---
    send(tx, BuildEvent::StepStarted("build-config-network".into()));
    ssh.set_step("build-config-network");
    ssh.detail("Configuring network IPs in config-rss.toml...").await;

    let gateway = &network.gateway;
    let dns_ip_0 = network.external_dns_ips.first().map(|s| s.as_str()).unwrap_or("192.168.2.40");
    let dns_ip_1 = network.external_dns_ips.get(1).map(|s| s.as_str()).unwrap_or("192.168.2.41");
    let svc_first = &network.internal_services_range.first;
    let svc_last = &network.internal_services_range.last;
    let infra_ip = &network.infra_ip;

    let rss_path = format!("{repo_path}/smf/sled-agent/non-gimlet/config-rss.toml");

    ssh.run_check(&format!(
        r#"sed -i 's/^external_dns_ips = .*/external_dns_ips = [ "{dns_ip_0}", "{dns_ip_1}" ]/' {rss_path}"#
    )).await?;
    ssh.run_check(&format!(
        r#"sed -i 's/^first = "192\.168\.[0-9]*\.[0-9]*"/first = "{svc_first}"/' {rss_path}"#
    )).await?;
    ssh.run_check(&format!(
        r#"sed -i 's/^last = "192\.168\.[0-9]*\.[0-9]*"/last = "{svc_last}"/' {rss_path}"#
    )).await?;
    ssh.run_check(&format!(
        r#"sed -i 's/^infra_ip_first = .*/infra_ip_first = "{infra_ip}"/' {rss_path}"#
    )).await?;
    ssh.run_check(&format!(
        r#"sed -i 's/^infra_ip_last = .*/infra_ip_last = "{infra_ip}"/' {rss_path}"#
    )).await?;
    ssh.run_check(&format!(
        r#"sed -i 's|address = "192\.168\.[0-9]*\.[0-9]*/24"|address = "{infra_ip}/24"|' {rss_path}"#
    )).await?;
    ssh.run_check(&format!(
        r#"sed -i 's/nexthop = "192\.168\.[0-9]*\.[0-9]*"/nexthop = "{gateway}"/' {rss_path}"#
    )).await?;

    ssh.detail("Verifying network config...").await;
    let rss_check = ssh.run(&format!(
        "grep -E 'external_dns_ips|^first|^last|infra_ip|nexthop|address.*192' {rss_path}"
    )).await?;
    ssh.detail(&format!("Network config:\n{}", rss_check.stdout.trim())).await;

    send(tx, BuildEvent::StepCompleted("build-config-network".into()));

    // --- Step: Apply source overrides (early — before cargo build compiles them) ---
    send(tx, BuildEvent::StepStarted("build-config-source".into()));
    ssh.set_step("build-config-source");
    ssh.detail("Applying source code overrides...").await;

    if let Some(crdb) = overrides.cockroachdb_redundancy {
        ssh.detail(&format!("Setting COCKROACHDB_REDUNDANCY = {crdb}...")).await;
        ssh.run_check(&format!(
            "sed -i 's/pub const COCKROACHDB_REDUNDANCY: usize = [0-9]*/pub const COCKROACHDB_REDUNDANCY: usize = {crdb}/' \
             {repo_path}/common/src/policy.rs"
        )).await?;
    }

    if let Some(buffer_gib) = overrides.control_plane_storage_buffer_gib {
        ssh.detail(&format!("Setting CONTROL_PLANE_STORAGE_BUFFER = {buffer_gib} GiB...")).await;
        ssh.run_check(&format!(
            "sed -i 's/ByteCount::from_gibibytes_u32([0-9]*)/ByteCount::from_gibibytes_u32({buffer_gib})/' \
             {repo_path}/nexus/src/app/mod.rs"
        )).await?;
    }

    send(tx, BuildEvent::StepCompleted("build-config-source".into()));

    // --- Step: Configure vdev count (early — before packaging bakes config.toml) ---
    send(tx, BuildEvent::StepStarted("build-config-vdevs".into()));
    ssh.set_step("build-config-vdevs");

    if let Some(vdev_count) = overrides.vdev_count {
        ssh.detail(&format!("Configuring {vdev_count} vdevs...")).await;

        // M.2 boot disks + U.2 data disks
        let mut vdev_entries: Vec<String> = vec![
            r#"    \"m2_0.vdev\","#.to_string(),
            r#"    \"m2_1.vdev\","#.to_string(),
        ];
        for i in 0..vdev_count {
            vdev_entries.push(format!(r#"    \"u2_{i}.vdev\","#));
        }
        let vdev_list = vdev_entries.join("\n");

        let expanded_repo = repo_path.replace("~", &format!("/home/{ssh_user}"));
        let config_toml_path = format!("{expanded_repo}/smf/sled-agent/non-gimlet/config.toml");
        ssh.run_check(&format!(
            r#"python3 -c "
import re
with open('{config_toml_path}', 'r') as f:
    content = f.read()
new_vdevs = '''vdevs = [
{vdev_list}
]'''
content = re.sub(r'vdevs\s*=\s*\[.*?\]', new_vdevs, content, flags=re.DOTALL)
with open('{config_toml_path}', 'w') as f:
    f.write(content)
print('Updated vdevs to {vdev_count}')
""#
        )).await?;
    } else {
        ssh.detail("Using default vdev count").await;
    }

    send(tx, BuildEvent::StepCompleted("build-config-vdevs".into()));

    // --- Step: Install builder prerequisites ---
    send(tx, BuildEvent::StepStarted("build-prereq-builder".into()));
    ssh.set_step("build-prereq-builder");
    ssh.detail("Installing builder prerequisites...").await;

    // Use proxy — the script runs cargo xtask download which fetches from buildomat via HTTPS
    ssh.run_streaming_check_with_proxy(&format!(
        "cd {repo_path} && bash -c '. ~/.cargo/env && source env.sh && \
         pfexec env PATH=$PATH ./tools/install_builder_prerequisites.sh -y' 2>&1"
    )).await.map_err(|e| {
        send(tx, BuildEvent::StepFailed("build-prereq-builder".into(), e.to_string()));
        e
    })?;

    send(tx, BuildEvent::StepCompleted("build-prereq-builder".into()));

    // --- Step: Install runner prerequisites ---
    send(tx, BuildEvent::StepStarted("build-prereq-runner".into()));
    ssh.set_step("build-prereq-runner");
    ssh.detail("Installing runner prerequisites...").await;

    // Use proxy — the script may download prebuilt artifacts via HTTPS
    ssh.run_streaming_check_with_proxy(&format!(
        "cd {repo_path} && bash -c '. ~/.cargo/env && source env.sh && \
         pfexec env PATH=$PATH ./tools/install_runner_prerequisites.sh -y' 2>&1"
    )).await.map_err(|e| {
        send(tx, BuildEvent::StepFailed("build-prereq-runner".into(), e.to_string()));
        e
    })?;

    send(tx, BuildEvent::StepCompleted("build-prereq-runner".into()));

    // --- Step: Fix file ownership ---
    send(tx, BuildEvent::StepStarted("build-fix-perms".into()));
    ssh.set_step("build-fix-perms");
    ssh.detail("Fixing file ownership...").await;

    ssh.run_check(&format!(
        "pfexec chown -R {ssh_user}:staff ~/.cargo ~/.rustup {repo_path}/target {repo_path}/out 2>/dev/null || true"
    )).await?;

    send(tx, BuildEvent::StepCompleted("build-fix-perms".into()));

    // --- Step: Build omicron-package ---
    send(tx, BuildEvent::StepStarted("build-compile".into()));
    ssh.set_step("build-compile");
    ssh.detail("Building omicron-package (this takes 45-60 min on first build)...").await;

    // Use proxy — cargo downloads crates from crates.io/github via HTTPS
    ssh.run_streaming_check_with_proxy(&format!(
        "cd {repo_path} && bash -c '. ~/.cargo/env && source env.sh && \
         cargo build --release -v --bin omicron-package' 2>&1"
    )).await.map_err(|e| {
        send(tx, BuildEvent::StepFailed("build-compile".into(), e.to_string()));
        e
    })?;

    ssh.detail("Creating packaging target...").await;
    ssh.run_check(&format!(
        "cd {repo_path} && bash -c '. ~/.cargo/env && source env.sh && \
         ./target/release/omicron-package -t default target create -p dev' 2>&1"
    )).await?;

    ssh.detail("Packaging all components...").await;
    // Use proxy — omicron-package downloads prebuilt binaries from buildomat via HTTPS
    // reqwest respects https_proxy + SSL_CERT_FILE env vars
    ssh.run_streaming_check_with_proxy(&format!(
        "cd {repo_path} && bash -c '. ~/.cargo/env && source env.sh && \
         ./target/release/omicron-package package' 2>&1"
    )).await.map_err(|e| {
        send(tx, BuildEvent::StepFailed("build-compile".into(), e.to_string()));
        e
    })?;

    send(tx, BuildEvent::StepCompleted("build-compile".into()));

    // --- Step: Patch propolis (download pre-built binary from GitHub release) ---
    send(tx, BuildEvent::StepStarted("build-patch-propolis".into()));
    ssh.set_step("build-patch-propolis");
    ssh.detail("Checking for patched propolis binary...").await;

    // Extract the propolis rev that omicron is pinned to
    // Use grep + sed (not GNU grep -P/-m which isn't available on illumos)
    let propolis_rev = ssh.run(&format!(
        "grep 'oxidecomputer/propolis.*rev=' {repo_path}/Cargo.lock | head -1 | sed 's/.*#//' | cut -c1-7"
    )).await?;
    let rev = propolis_rev.stdout.trim().to_string();

    if !rev.is_empty() {
        let release_tag = format!("patched-{rev}");
        ssh.detail(&format!("Propolis pinned at {rev}, checking for release {release_tag}...")).await;

        // Check if the release exists and download
        let download_result = ssh.run(&format!(
            "curl -sfL -o /tmp/propolis-server.gz \
             https://github.com/swherdman/propolis/releases/download/{release_tag}/propolis-server.gz \
             && echo OK || echo MISSING"
        )).await?;

        if download_result.stdout.trim() == "OK" {
            ssh.detail("Patched binary downloaded, swapping into tarball...").await;

            // Decompress, extract tarball, swap binary, repack
            ssh.run_check(&format!(
                "cd /tmp && \
                 gunzip -f propolis-server.gz && \
                 chmod +x propolis-server && \
                 mkdir -p propolis-repack && cd propolis-repack && \
                 tar xzf {repo_path}/out/propolis-server.tar.gz && \
                 cp /tmp/propolis-server root/opt/oxide/propolis-server/bin/propolis-server && \
                 tar czf {repo_path}/out/propolis-server.tar.gz oxide.json root/ && \
                 cd /tmp && rm -rf propolis-repack propolis-server"
            )).await?;

            ssh.detail(&format!("Propolis patched with release {release_tag}")).await;
        } else {
            ssh.detail(&format!(
                "Warning: no patched release for {release_tag} — VMs may crash on string I/O"
            )).await;
        }
    } else {
        ssh.detail("Warning: could not determine propolis rev from Cargo.lock").await;
    }

    send(tx, BuildEvent::StepCompleted("build-patch-propolis".into()));

    // --- Step: Create virtual hardware ---
    send(tx, BuildEvent::StepStarted("build-vhw".into()));
    ssh.set_step("build-vhw");
    ssh.detail("Creating virtual hardware...").await;

    let vdev_size = overrides.vdev_size_bytes.unwrap_or(42949672960);
    let pxa_start = &network.internal_services_range.first;
    let pxa_end = &network.instance_pool_range.last;

    ssh.run_check(&format!(
        "cd {repo_path} && bash -c '. ~/.cargo/env && source env.sh && \
         pfexec cargo xtask virtual-hardware create \
         --gateway-ip {gateway} \
         --pxa-start {pxa_start} \
         --pxa-end {pxa_end} \
         --vdev-size {vdev_size}' 2>&1"
    )).await.map_err(|e| {
        send(tx, BuildEvent::StepFailed("build-vhw".into(), e.to_string()));
        e
    })?;

    send(tx, BuildEvent::StepCompleted("build-vhw".into()));

    // --- Step: Install + wait for zones ---
    send(tx, BuildEvent::StepStarted("build-install".into()));
    ssh.set_step("build-install");
    ssh.detail("Installing omicron (this takes 5-8 minutes)...").await;

    ssh.run_check(&format!(
        "cd {repo_path} && bash -c '. ~/.cargo/env && source env.sh && \
         pfexec ./target/release/omicron-package install' 2>&1"
    )).await.map_err(|e| {
        send(tx, BuildEvent::StepFailed("build-install".into(), e.to_string()));
        e
    })?;

    // Wait for zones to come up
    let expected_zones = crate::config::derive_expected_zones(overrides);
    let expected_total: u32 = expected_zones.values().sum();
    // Add 2 for global + sidecar zones that aren't in the service count
    let expected_running = expected_total + 2;

    ssh.detail(&format!("Waiting for zones ({expected_running} expected)...")).await;

    for attempt in 0..60 {
        let zone_output = ssh.run("zoneadm list -cnv | grep running | wc -l").await?;
        let running: u32 = zone_output.stdout.trim().parse().unwrap_or(0);

        send(tx, BuildEvent::StepDetail(
            "build-install".into(),
            format!("Zones: {running}/{expected_running} running"),
        ));

        if running >= expected_running {
            ssh.detail(&format!("All {running} zones running")).await;
            break;
        }

        if attempt == 59 {
            ssh.detail(&format!("Warning: only {running}/{expected_running} zones after 5 min")).await;
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    send(tx, BuildEvent::StepCompleted("build-install".into()));

    // --- Step: Verify DNS + API ---
    send(tx, BuildEvent::StepStarted("build-verify".into()));
    ssh.set_step("build-verify");
    ssh.detail("Verifying DNS and API...").await;

    let dns_ip = network.external_dns_ips.first()
        .map(|s| s.as_str())
        .unwrap_or("192.168.2.70");

    // Wait for DNS to resolve
    let mut dns_ok = false;
    for _ in 0..30 {
        let dns_check = ssh.run(&format!(
            "dig recovery.sys.oxide.test @{dns_ip} +short +time=3 +tries=1 2>/dev/null"
        )).await?;
        if dns_check.exit_code == 0 && !dns_check.stdout.trim().is_empty() {
            ssh.detail(&format!("DNS resolving: {}", dns_check.stdout.trim())).await;
            dns_ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    if !dns_ok {
        ssh.detail("Warning: DNS not resolving yet").await;
    }

    // Check Nexus API
    let nexus_ip = if dns_ok {
        let dns_result = ssh.run(&format!(
            "dig recovery.sys.oxide.test @{dns_ip} +short 2>/dev/null"
        )).await?;
        dns_result.stdout.trim().to_string()
    } else {
        // Fall back to first IP in services range
        network.internal_services_range.first.clone()
    };

    let mut api_ok = false;
    for _ in 0..12 {
        let ping = ssh.run(&format!(
            "curl -sf --connect-timeout 3 --max-time 5 http://{nexus_ip}/v1/ping 2>/dev/null"
        )).await?;
        if ping.exit_code == 0 {
            ssh.detail(&format!("Nexus API responding at {nexus_ip}")).await;
            api_ok = true;
            break;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    if !api_ok {
        ssh.detail("Warning: Nexus API not responding yet").await;
    }

    send(tx, BuildEvent::StepCompleted("build-verify".into()));

    // --- Step: Set silo quotas ---
    send(tx, BuildEvent::StepStarted("build-quotas".into()));
    ssh.set_step("build-quotas");
    ssh.detail("Setting silo quotas...").await;

    let nexus = &config.deployment.nexus;

    // Authenticate via direct API call (skip oxide CLI device auth)
    let auth_cmd = format!(
        "curl -sf -X POST http://{nexus_ip}/v1/login/{}/local \
         -H 'Content-Type: application/json' \
         -d '{{\"username\":\"{}\",\"password\":\"{}\"}}' \
         -c /tmp/oxide-cookie 2>/dev/null",
        nexus.silo_name, nexus.username, nexus.password
    );
    let auth_result = ssh.run(&auth_cmd).await?;
    if auth_result.exit_code != 0 {
        ssh.detail("Warning: Nexus auth failed, quotas may not be set").await;
    } else {
        // Set quotas
        let quota_cmd = format!(
            "curl -sf -X PUT http://{nexus_ip}/v1/system/silos/{}/quotas \
             -H 'Content-Type: application/json' \
             -b /tmp/oxide-cookie \
             -d '{{\"cpus\":{},\"memory\":{},\"storage\":{}}}' 2>/dev/null",
            nexus.silo_name, nexus.quotas.cpus, nexus.quotas.memory, nexus.quotas.storage
        );
        let _ = ssh.run(&quota_cmd).await;
        ssh.detail("Silo quotas set").await;
    }

    send(tx, BuildEvent::StepCompleted("build-quotas".into()));

    // --- Step: Create IP pool ---
    send(tx, BuildEvent::StepStarted("build-ippool".into()));
    ssh.set_step("build-ippool");
    ssh.detail("Creating IP pool...").await;

    if auth_result.exit_code == 0 {
        let pool_name = &nexus.ip_pool_name;
        let pool_first = &network.instance_pool_range.first;
        let pool_last = &network.instance_pool_range.last;

        // Create pool
        let _ = ssh.run(&format!(
            "curl -sf -X POST http://{nexus_ip}/v1/ip-pools \
             -H 'Content-Type: application/json' \
             -b /tmp/oxide-cookie \
             -d '{{\"name\":\"{pool_name}\",\"description\":\"Default IP pool\"}}' 2>/dev/null"
        )).await;

        // Link to silo
        let _ = ssh.run(&format!(
            "curl -sf -X POST http://{nexus_ip}/v1/ip-pools/{pool_name}/silos \
             -H 'Content-Type: application/json' \
             -b /tmp/oxide-cookie \
             -d '{{\"silo\":\"{}\",\"is_default\":true}}' 2>/dev/null",
            nexus.silo_name
        )).await;

        // Add IP range
        let _ = ssh.run(&format!(
            "curl -sf -X POST http://{nexus_ip}/v1/ip-pools/{pool_name}/ranges/add \
             -H 'Content-Type: application/json' \
             -b /tmp/oxide-cookie \
             -d '{{\"first\":\"{pool_first}\",\"last\":\"{pool_last}\"}}' 2>/dev/null"
        )).await;

        ssh.detail(&format!("IP pool '{pool_name}' created with range {pool_first}-{pool_last}")).await;
    } else {
        ssh.detail("Skipped — auth failed earlier").await;
    }

    send(tx, BuildEvent::StepCompleted("build-ippool".into()));

    let _ = helios.close().await;
    Ok(())
}

/// Poll SSH connectivity until it comes back after a reboot.
async fn wait_for_ssh(ip: &str, user: &str, timeout: Duration) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    let config = HostConfig {
        address: ip.to_string(),
        ssh_user: user.to_string(),
        role: crate::config::HostRole::Combined,
    };

    loop {
        if tokio::time::Instant::now() > deadline {
            return Err(eyre!("SSH did not come back within {}s", timeout.as_secs()));
        }

        match DirectSsh::connect(&config).await {
            Ok(host) => {
                let _ = host.close().await;
                return Ok(());
            }
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

/// Set up local caching proxies and configure the Helios host to use them.
/// - nginx: reverse proxy for pkg.oxide.computer (IPS packages)
/// - squid: forward proxy with SSL-bump for HTTPS downloads (xtask, rustup, etc.)
async fn run_setup_pkg_cache(
    helios_ip: &str,
    ssh_user: &str,
    tx: &mpsc::UnboundedSender<BuildEvent>,
) -> Result<()> {
    send(tx, BuildEvent::StepStarted("build-pkg-cache".into()));
    send(
        tx,
        BuildEvent::StepDetail("build-pkg-cache".into(), "Starting local caching proxies...".into()),
    );

    // Ensure both Docker containers are running
    let cache_info = crate::ops::pkg_cache::ensure_caches().await.map_err(|e| {
        send(tx, BuildEvent::StepFailed("build-pkg-cache".into(), e.to_string()));
        e
    })?;

    let nginx_status = if cache_info.nginx_was_running { "already running" } else { "started" };
    let squid_status = if cache_info.squid_was_running { "already running" } else { "started" };

    send(
        tx,
        BuildEvent::StepDetail(
            "build-pkg-cache".into(),
            format!("nginx {nginx_status}, squid {squid_status} on {}", cache_info.lan_ip),
        ),
    );

    // Connect to Helios host
    let helios_config = HostConfig {
        address: helios_ip.to_string(),
        ssh_user: ssh_user.to_string(),
        role: crate::config::HostRole::Combined,
    };
    let helios = DirectSsh::connect(&helios_config).await.map_err(|e| {
        send(tx, BuildEvent::StepFailed("build-pkg-cache".into(), format!("SSH failed: {e}")));
        e
    })?;

    // Verify pkg cache reachability
    send(
        tx,
        BuildEvent::StepDetail("build-pkg-cache".into(), "Verifying pkg cache from Helios...".into()),
    );

    let pkg_ok = crate::ops::pkg_cache::verify_pkg_cache(&helios, &cache_info.publisher_url).await?;
    if !pkg_ok {
        let _ = helios.close().await;
        let msg = format!("Pkg cache not reachable at {}", cache_info.publisher_url);
        send(tx, BuildEvent::StepFailed("build-pkg-cache".into(), msg.clone()));
        return Err(eyre!("{msg}"));
    }

    // Set pkg publisher
    send(
        tx,
        BuildEvent::StepDetail("build-pkg-cache".into(), "Setting pkg publisher...".into()),
    );
    crate::ops::pkg_cache::set_publisher(&helios, &cache_info.publisher_url).await.map_err(|e| {
        send(tx, BuildEvent::StepFailed("build-pkg-cache".into(), e.to_string()));
        e
    })?;

    // Install CA cert for HTTPS proxy
    send(
        tx,
        BuildEvent::StepDetail("build-pkg-cache".into(), "Installing HTTPS proxy CA cert...".into()),
    );
    let ca_cert_path = crate::ops::pkg_cache::install_ca_cert(&helios, &cache_info.lan_ip).await.map_err(|e| {
        send(tx, BuildEvent::StepFailed("build-pkg-cache".into(), format!("CA cert install failed: {e}")));
        e
    })?;

    // Configure proxy environment variables
    send(
        tx,
        BuildEvent::StepDetail("build-pkg-cache".into(), "Configuring HTTPS proxy env...".into()),
    );
    crate::ops::pkg_cache::configure_proxy_env(&helios, &cache_info.https_proxy_url, &ca_cert_path).await.map_err(|e| {
        send(tx, BuildEvent::StepFailed("build-pkg-cache".into(), format!("Proxy env config failed: {e}")));
        e
    })?;

    // Verify HTTPS proxy works
    send(
        tx,
        BuildEvent::StepDetail("build-pkg-cache".into(), "Verifying HTTPS proxy from Helios...".into()),
    );
    let proxy_ok = crate::ops::pkg_cache::verify_https_proxy(&helios, &cache_info.https_proxy_url).await?;
    if !proxy_ok {
        // Non-fatal — HTTPS proxy is a nice-to-have, pkg cache is the critical one
        send(
            tx,
            BuildEvent::StepDetail(
                "build-pkg-cache".into(),
                "Warning: HTTPS proxy not reachable — downloads will go direct".into(),
            ),
        );
    }

    let _ = helios.close().await;

    send(
        tx,
        BuildEvent::StepDetail(
            "build-pkg-cache".into(),
            format!(
                "Caches configured: pkg={} https_proxy={}",
                cache_info.publisher_url, cache_info.https_proxy_url
            ),
        ),
    );
    send(tx, BuildEvent::StepCompleted("build-pkg-cache".into()));

    Ok(())
}

fn send(tx: &mpsc::UnboundedSender<BuildEvent>, event: BuildEvent) {
    let _ = tx.send(event);
}
