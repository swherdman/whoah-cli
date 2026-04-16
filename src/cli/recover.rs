use std::io::{self, Write};

use color_eyre::Result;
use tokio_util::sync::CancellationToken;

use crate::config;
use crate::ops::recover::{RecoveryEvent, RecoveryParams, RecoveryStep, run_recovery};
use crate::ops::status::{gather_status, is_post_reboot};
use crate::ssh::session::SshHost;

pub async fn run(deployment: Option<&str>) -> Result<()> {
    let deployment_name = config::resolve_deployment(deployment)?;
    let cfg = config::load_deployment(&deployment_name)?;

    let host_config = cfg.deployment.hosts.values().next().ok_or_else(|| {
        color_eyre::eyre::eyre!("No hosts configured in deployment '{deployment_name}'")
    })?;

    eprintln!(
        "Connecting to {}@{}...",
        host_config.ssh_user, host_config.address
    );

    let host = SshHost::connect(host_config).await?;

    // Check if recovery is needed
    eprintln!("Checking system state...");
    let status = gather_status(&host, &cfg).await?;

    if !is_post_reboot(&status) {
        eprintln!(
            "System appears healthy ({} zones running, simnets present). Recovery not needed.",
            status.zones.service_counts.values().sum::<u32>()
        );
        eprintln!("Run with --force to recover anyway (not yet implemented).");
        host.close().await?;
        return Ok(());
    }

    eprintln!("Reboot detected. Starting recovery...\n");

    let params = RecoveryParams::from_config(&cfg)?;
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let cancel = CancellationToken::new();

    // Spawn recovery in background task
    let host_ref: &dyn crate::ssh::RemoteHost = &host;
    // We need to run recovery and event printing concurrently
    // Use a scope approach: run recovery to completion while draining events
    let params_clone = params.clone();
    let cancel_clone = cancel.clone();

    // Since we can't send &host across spawn boundary, run in the same task
    // using select to drain events while recovery runs
    let (result, _) = tokio::join!(
        async { run_recovery(host_ref, &params_clone, tx, cancel_clone).await },
        async {
            while let Some(event) = rx.recv().await {
                print_event(&event);
            }
        }
    );

    if let Err(e) = &result {
        eprintln!("\nRecovery failed: {e}");
        // TODO: prompt for workaround in interactive mode
    }

    host.close().await?;
    result
}

fn print_event(event: &RecoveryEvent) {
    match event {
        RecoveryEvent::StepStarted(step) => {
            eprintln!(
                "[{}/{}] {} ...",
                step.index() + 1,
                RecoveryStep::total_count(),
                step.label()
            );
        }
        RecoveryEvent::StepOutput(line) => {
            eprintln!("      {line}");
        }
        RecoveryEvent::ZoneProgress { running, expected } => {
            eprint!("\r      Zones: {running}/{expected} running");
            let _ = io::stderr().flush();
            if running >= expected {
                eprintln!();
            }
        }
        RecoveryEvent::StepCompleted(step, duration) => {
            eprintln!("  OK  {} ({:.1}s)", step.label(), duration.as_secs_f64());
        }
        RecoveryEvent::StepFailed {
            step,
            error,
            workaround,
        } => {
            eprintln!("  FAIL {} : {error}", step.label());
            if let Some(w) = workaround {
                eprintln!("  HINT: {}", w.description());
            }
        }
        RecoveryEvent::RecoveryComplete(duration) => {
            eprintln!("\nRecovery complete in {:.0}s", duration.as_secs_f64());
        }
    }
}
