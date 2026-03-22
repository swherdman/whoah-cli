//! Global SSH session registry.
//!
//! Tracks all active SSH sessions with their state for the debug view.
//! Sessions register on connect and unregister on close/drop.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};


/// A point-in-time snapshot of an SSH session's state.
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    pub id: String,
    pub destination: String,
    pub label: String,
    pub ctl_path: PathBuf,
    pub log_path: PathBuf,
    pub socket_exists: bool,
    pub connected_at: Instant,
    pub uptime: Duration,
    pub command_count: u64,
    pub last_command: Option<String>,
    pub last_command_ago: Option<Duration>,
    pub master_log_tail: Option<String>,
}

static REGISTRY: OnceLock<Mutex<HashMap<String, SessionEntry>>> = OnceLock::new();

struct SessionEntry {
    destination: String,
    label: String,
    ctl_path: PathBuf,
    log_path: PathBuf,
    connected_at: Instant,
    command_count: u64,
    last_command: Option<String>,
    last_command_at: Option<Instant>,
}

fn registry() -> &'static Mutex<HashMap<String, SessionEntry>> {
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Register a new session.
pub fn register(id: &str, destination: &str, ctl_path: PathBuf, log_path: PathBuf) {
    let entry = SessionEntry {
        destination: destination.to_string(),
        label: String::new(),
        ctl_path,
        log_path,
        connected_at: Instant::now(),
        command_count: 0,
        last_command: None,
        last_command_at: None,
    };
    if let Ok(mut reg) = registry().lock() {
        reg.insert(id.to_string(), entry);
    }
}

/// Remove a session from the registry.
pub fn unregister(id: &str) {
    if let Ok(mut reg) = registry().lock() {
        reg.remove(id);
    }
}

/// Set a label on a session (e.g., "Monitor", "Build/Proxmox").
pub fn set_label(id: &str, label: &str) {
    if let Ok(mut reg) = registry().lock() {
        if let Some(entry) = reg.get_mut(id) {
            entry.label = label.to_string();
        }
    }
}

/// Record that a command was executed on a session.
pub fn record_command(id: &str, cmd: &str) {
    if let Ok(mut reg) = registry().lock() {
        if let Some(entry) = reg.get_mut(id) {
            entry.command_count += 1;
            entry.last_command = Some(cmd.chars().take(80).collect());
            entry.last_command_at = Some(Instant::now());
        }
    }
}

/// Get snapshots of all registered sessions.
pub fn all() -> Vec<SessionSnapshot> {
    let reg = match registry().lock() {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let now = Instant::now();
    reg.iter()
        .map(|(id, entry)| {
            let socket_exists = entry.ctl_path.exists();
            let master_log_tail = if !socket_exists {
                // Only read log when something is wrong
                read_log_tail(&entry.log_path, 3)
            } else {
                None
            };

            SessionSnapshot {
                id: id.clone(),
                destination: entry.destination.clone(),
                label: entry.label.clone(),
                ctl_path: entry.ctl_path.clone(),
                log_path: entry.log_path.clone(),
                socket_exists,
                connected_at: entry.connected_at,
                uptime: now.duration_since(entry.connected_at),
                command_count: entry.command_count,
                last_command: entry.last_command.clone(),
                last_command_ago: entry.last_command_at.map(|t| now.duration_since(t)),
                master_log_tail,
            }
        })
        .collect()
}

/// Read the last N lines of a file.
fn read_log_tail(path: &PathBuf, lines: usize) -> Option<String> {
    let content = std::fs::read_to_string(path).ok()?;
    let tail: Vec<&str> = content.lines().rev().take(lines).collect();
    if tail.is_empty() {
        None
    } else {
        let result: Vec<&str> = tail.into_iter().rev().collect();
        Some(result.join("\n"))
    }
}
