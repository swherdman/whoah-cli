//! Zone-recovery watchdog.
//!
//! Runs as a background task alongside `deploy-verify`. When a known zone
//! first appears running, the watchdog applies hardcoded recovery actions to
//! fix SMF services that are stuck or need a one-shot enable.
//!
//! To add a fix for a newly discovered zone/service issue, add an entry to
//! `BUILTIN_RULES`. The operator-facing kill switch is
//! `tuning.zone_watchdog_enabled` in build.toml.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::config::HostConfig;
use crate::event::BuildEvent;
use crate::ssh::session::SshHost;

struct WatchdogRule {
    /// Identifier shown in StepDetail lines (e.g. "ntp_zone").
    name: &'static str,
    /// Substring matched against zone names from `zoneadm list -cp`.
    zone_substring: &'static str,
    /// Commands run via `pfexec zlogin <zone> '<cmd>'` once when the zone
    /// first appears running. Must not contain a single-quote character.
    on_appear: &'static [&'static str],
    /// SMF FMRIs checked each iteration; if state == "maintenance",
    /// `svcadm clear <fmri>` is run inside the zone.
    clear_if_maintenance: &'static [&'static str],
    /// SMF FMRIs polled for "online" state. Emits one StepDetail on
    /// transition; does not block the verify loop.
    wait_online: &'static [&'static str],
}

/// All built-in recovery rules. Extend here when new zone/SMF issues are found.
static BUILTIN_RULES: &[WatchdogRule] = &[WatchdogRule {
    name: "ntp_zone",
    zone_substring: "oxz_ntp",
    // Failure B (unconditional): oxide/ntp has a hard require_all dep on
    // ndp, but ndp is disabled in the NTP zone profile. Always enable it.
    on_appear: &["svcadm enable network/routing/ndp"],
    // Failure A (conditional, I/O-triggered): ipmgmtd times out and
    // lands in maintenance under heavy concurrent I/O.
    clear_if_maintenance: &["network/ip-interface-management"],
    wait_online: &["oxide/ntp"],
}];

/// Spawn as a background task alongside the `deploy-verify` loop.
/// Monitors zone appearances and applies SMF recovery actions for known issues.
/// Emits `BuildEvent::StepDetail` under step id `"deploy-verify"`.
/// The caller must abort the returned JoinHandle when verify completes.
pub async fn run_zone_watchdog(
    helios_config: HostConfig,
    log_path: PathBuf,
    tx: mpsc::UnboundedSender<BuildEvent>,
) {
    let host = match SshHost::connect(&helios_config).await {
        Ok(h) => h,
        Err(e) => {
            let _ = tx.send(BuildEvent::StepDetail(
                "deploy-verify".into(),
                format!("watchdog: failed to connect: {e}; NTP zone recovery disabled"),
            ));
            return;
        }
    };
    host.set_label("Watchdog");

    let mut ssh = match crate::ops::ssh_log::LoggedSsh::new(
        &host,
        log_path,
        &tx,
        "deploy-verify",
        "Watchdog",
    )
    .await
    {
        Ok(s) => s,
        Err(e) => {
            let _ = tx.send(BuildEvent::StepDetail(
                "deploy-verify".into(),
                format!("watchdog: failed to open log: {e}; NTP zone recovery disabled"),
            ));
            return;
        }
    };

    let _ = tx.send(BuildEvent::StepDetail(
        "deploy-verify".into(),
        "watchdog: active".into(),
    ));

    // Keyed by (rule_name, zone_name) to ensure one-shot semantics.
    let mut on_appear_done: HashSet<(&'static str, String)> = HashSet::new();
    // Keyed by (zone_name, fmri) — stop re-checking once cleared or healthy.
    let mut clear_done: HashSet<(String, &'static str)> = HashSet::new();
    // Keyed by (zone_name, fmri) — emit online event once per fmri.
    let mut online_seen: HashSet<(String, &'static str)> = HashSet::new();

    loop {
        // Poll zone list — file only (run_quiet), not TUI.
        let zone_out = match ssh.run_quiet("zoneadm list -cp").await {
            Ok(o) => o,
            Err(_) => {
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        // zoneadm -cp format: id:name:state:...
        // Field 1 (0-indexed) is the zone name; field 2 is the state.
        let running: Vec<String> = zone_out
            .stdout
            .lines()
            .filter(|l: &&str| l.contains(":running:"))
            .filter_map(|l: &str| l.split(':').nth(1).map(str::to_owned))
            .collect();

        for rule in BUILTIN_RULES {
            for zone in running
                .iter()
                .filter(|z: &&String| z.contains(rule.zone_substring))
            {
                // Validate zone name before embedding in shell commands. Illumos zone
                // names are restricted to [a-zA-Z0-9_-.] — anything else is unexpected
                // and could break the single-quoted zlogin invocation.
                if !zone_name_is_safe(zone) {
                    let _ = tx.send(BuildEvent::StepDetail(
                        "deploy-verify".into(),
                        format!(
                            "watchdog [{}]: skipping zone with unexpected name: {zone:?}",
                            rule.name
                        ),
                    ));
                    continue;
                }

                // --- 1. on_appear (one-shot per zone) ---
                let appear_key = (rule.name, zone.clone());
                if !on_appear_done.contains(&appear_key) {
                    for cmd in rule.on_appear {
                        let full = format!("pfexec zlogin {zone} '{cmd}'");
                        // run_quiet: full I/O goes to log file; human-readable summary to TUI.
                        match ssh.run_quiet(&full).await {
                            Ok(r) if r.exit_code == 0 => {
                                let _ = tx.send(BuildEvent::StepDetail(
                                    "deploy-verify".into(),
                                    format!("watchdog [{}]: {cmd} in {zone}", rule.name),
                                ));
                            }
                            Ok(r) => {
                                let _ = tx.send(BuildEvent::StepDetail(
                                    "deploy-verify".into(),
                                    format!(
                                        "watchdog [{}]: {cmd} in {zone} exited {} — {}",
                                        rule.name,
                                        r.exit_code,
                                        r.stderr.trim()
                                    ),
                                ));
                            }
                            Err(e) => {
                                let _ = tx.send(BuildEvent::StepDetail(
                                    "deploy-verify".into(),
                                    format!(
                                        "watchdog [{}]: {cmd} in {zone} failed: {e}",
                                        rule.name
                                    ),
                                ));
                            }
                        }
                    }
                    on_appear_done.insert(appear_key);
                }

                // --- 2. clear_if_maintenance (re-checked each iteration until done) ---
                for fmri in rule.clear_if_maintenance {
                    let key = (zone.clone(), *fmri);
                    if clear_done.contains(&key) {
                        continue;
                    }

                    // State query — file only.
                    let state_cmd = format!(
                        "pfexec zlogin {zone} 'svcs -H -o state {fmri} 2>/dev/null || true'"
                    );
                    let state = ssh
                        .run_quiet(&state_cmd)
                        .await
                        .map(|r| r.stdout.trim().to_string())
                        .unwrap_or_default();

                    match state.as_str() {
                        // "offline*" is the compound offline+maintenance state that
                        // svcs -H -o state reports when a service is offline waiting
                        // on a dependency that is in maintenance.
                        "maintenance" | "offline*" => {
                            // Capture svcs -xv for forensic diagnosis before clearing.
                            // run() sends the command to TUI so the operator sees it.
                            let xv_cmd = format!("pfexec zlogin {zone} 'svcs -xv {fmri}'");
                            let _ = ssh.run(&xv_cmd).await;

                            // Clear the service — file only; human summary goes to TUI below.
                            let clear_cmd = format!("pfexec zlogin {zone} 'svcadm clear {fmri}'");
                            let _ = ssh.run_quiet(&clear_cmd).await;
                            let _ = tx.send(BuildEvent::StepDetail(
                                "deploy-verify".into(),
                                format!(
                                    "watchdog [{}]: cleared {fmri} in {zone} (was {state})",
                                    rule.name
                                ),
                            ));
                            clear_done.insert(key);
                        }
                        "online" => {
                            clear_done.insert(key);
                        }
                        _ => {}
                    }
                }

                // --- 3. wait_online (observe once per fmri) ---
                for fmri in rule.wait_online {
                    let key = (zone.clone(), *fmri);
                    if online_seen.contains(&key) {
                        continue;
                    }

                    // State query — file only.
                    let state_cmd = format!(
                        "pfexec zlogin {zone} 'svcs -H -o state {fmri} 2>/dev/null || true'"
                    );
                    if let Ok(r) = ssh.run_quiet(&state_cmd).await
                        && r.stdout.trim() == "online"
                    {
                        let _ = tx.send(BuildEvent::StepDetail(
                            "deploy-verify".into(),
                            format!("watchdog [{}]: {fmri} online in {zone}", rule.name),
                        ));
                        online_seen.insert(key);
                    }
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

fn zone_name_is_safe(name: &str) -> bool {
    !name
        .chars()
        .any(|c| !c.is_ascii_alphanumeric() && !matches!(c, '_' | '-' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_name_validation_rejects_unsafe_chars() {
        assert!(!zone_name_is_safe("oxz_ntp'0"));
        assert!(!zone_name_is_safe("zone name"));
        assert!(!zone_name_is_safe("zone;rm -rf /"));
        assert!(zone_name_is_safe("oxz_ntp_1a2b3c4d"));
        assert!(zone_name_is_safe("oxz_switch_00000001"));
        assert!(zone_name_is_safe("zone-name.test"));
    }

    #[test]
    fn builtin_rules_no_single_quotes_in_commands() {
        for rule in BUILTIN_RULES {
            for cmd in rule.on_appear {
                assert!(
                    !cmd.contains('\''),
                    "Rule '{}' on_appear command contains single quote: {cmd}",
                    rule.name
                );
            }
            for fmri in rule.clear_if_maintenance {
                assert!(
                    !fmri.contains('\''),
                    "Rule '{}' clear_if_maintenance fmri contains single quote: {fmri}",
                    rule.name
                );
            }
            for fmri in rule.wait_online {
                assert!(
                    !fmri.contains('\''),
                    "Rule '{}' wait_online fmri contains single quote: {fmri}",
                    rule.name
                );
            }
        }
    }
}
