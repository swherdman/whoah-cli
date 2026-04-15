//! Build & deploy pipeline data model.
//!
//! Defines the phased pipeline structure for provisioning a Proxmox VM,
//! setting up Helios, building Omicron, and applying patches. Each step
//! tracks status and timing. No execution logic lives here — this is
//! purely the state that the TUI renders and that executors update.

use std::time::{Duration, Instant};

// ── Status ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum StepStatus {
    Pending,
    Running {
        started: Instant,
        detail: Option<String>,
    },
    Completed {
        duration: Duration,
    },
    Failed {
        duration: Duration,
        error: String,
    },
}

impl StepStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            StepStatus::Completed { .. } | StepStatus::Failed { .. }
        )
    }
}

// ── Step ────────────────────────────────────────────────────────

/// Maximum number of output lines to retain per step for the log panel.
const STEP_OUTPUT_CAPACITY: usize = 200;

#[derive(Debug, Clone)]
pub struct Step {
    pub id: &'static str,
    pub name: &'static str,
    pub status: StepStatus,
    /// Recent output lines for the log panel. Capped at STEP_OUTPUT_CAPACITY.
    pub output: Vec<String>,
}

impl Step {
    fn new(id: &'static str, name: &'static str) -> Self {
        Self {
            id,
            name,
            status: StepStatus::Pending,
            output: Vec::new(),
        }
    }

    /// Push a line to the output buffer, trimming if over capacity.
    pub fn push_output(&mut self, line: String) {
        if self.output.len() >= STEP_OUTPUT_CAPACITY {
            self.output.remove(0);
        }
        self.output.push(line);
    }

    pub fn elapsed(&self) -> Option<Duration> {
        match &self.status {
            StepStatus::Running { started, .. } => Some(started.elapsed()),
            StepStatus::Completed { duration } => Some(*duration),
            StepStatus::Failed { duration, .. } => Some(*duration),
            _ => None,
        }
    }

    pub fn detail(&self) -> Option<&str> {
        match &self.status {
            StepStatus::Running { detail, .. } => detail.as_deref(),
            StepStatus::Failed { error, .. } => Some(error.as_str()),
            _ => None,
        }
    }
}

// ── Phase ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Phase {
    pub name: &'static str,
    pub steps: Vec<Step>,
}

impl Phase {
    fn new(name: &'static str, steps: Vec<Step>) -> Self {
        Self { name, steps }
    }

    pub fn elapsed(&self) -> Duration {
        self.steps.iter().filter_map(|s| s.elapsed()).sum()
    }

    pub fn is_complete(&self) -> bool {
        self.steps.iter().all(|s| s.status.is_terminal())
    }

    pub fn is_pending(&self) -> bool {
        self.steps
            .iter()
            .all(|s| matches!(s.status, StepStatus::Pending))
    }

    pub fn has_failure(&self) -> bool {
        self.steps
            .iter()
            .any(|s| matches!(s.status, StepStatus::Failed { .. }))
    }
}

// ── Pipeline ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Pipeline {
    pub phases: Vec<Phase>,
    pub started: Option<Instant>,
}

impl Pipeline {
    pub fn total_elapsed(&self) -> Duration {
        self.started.map(|s| s.elapsed()).unwrap_or(Duration::ZERO)
    }

    /// Find a step by id across all phases. Returns (phase_index, step_index).
    pub fn find_step(&self, id: &str) -> Option<(usize, usize)> {
        for (pi, phase) in self.phases.iter().enumerate() {
            for (si, step) in phase.steps.iter().enumerate() {
                if step.id == id {
                    return Some((pi, si));
                }
            }
        }
        None
    }

    /// Get a mutable reference to a step by id.
    pub fn step_mut(&mut self, id: &str) -> Option<&mut Step> {
        for phase in &mut self.phases {
            for step in &mut phase.steps {
                if step.id == id {
                    return Some(step);
                }
            }
        }
        None
    }

    /// Mark a step as running.
    pub fn start_step(&mut self, id: &str) {
        if self.started.is_none() {
            self.started = Some(Instant::now());
        }
        if let Some(step) = self.step_mut(id) {
            step.status = StepStatus::Running {
                started: Instant::now(),
                detail: None,
            };
        }
    }

    /// Mark a step as completed.
    pub fn complete_step(&mut self, id: &str) {
        if let Some(step) = self.step_mut(id) {
            let duration = match &step.status {
                StepStatus::Running { started, .. } => started.elapsed(),
                _ => Duration::ZERO,
            };
            step.status = StepStatus::Completed { duration };
        }
    }

    /// Mark a step as failed.
    pub fn fail_step(&mut self, id: &str, error: String) {
        if let Some(step) = self.step_mut(id) {
            let duration = match &step.status {
                StepStatus::Running { started, .. } => started.elapsed(),
                _ => Duration::ZERO,
            };
            step.status = StepStatus::Failed { duration, error };
        }
    }

    pub fn is_complete(&self) -> bool {
        self.phases.iter().all(|p| p.is_complete())
    }

    pub fn has_failure(&self) -> bool {
        self.phases.iter().any(|p| p.has_failure())
    }

    /// Return (completed_steps, total_steps).
    pub fn progress(&self) -> (usize, usize) {
        let total: usize = self.phases.iter().map(|p| p.steps.len()).sum();
        let done: usize = self
            .phases
            .iter()
            .flat_map(|p| &p.steps)
            .filter(|s| s.status.is_terminal())
            .count();
        (done, total)
    }
}

// ── Factory ─────────────────────────────────────────────────────

/// Create the full build & deploy pipeline with all phases and steps.
pub fn build_deploy_pipeline() -> Pipeline {
    Pipeline {
        started: None,
        phases: vec![
            Phase::new(
                "Provision VM",
                vec![
                    Step::new("prov-create", "Create VM"),
                    Step::new("prov-boot", "Boot ISO"),
                    Step::new("prov-install", "Install Helios"),
                ],
            ),
            Phase::new(
                "Configure VM",
                vec![
                    Step::new("vm-network", "Configure networking"),
                    Step::new("vm-netcat", "Start netcat listener"),
                ],
            ),
            Phase::new(
                "Configure Access",
                vec![
                    Step::new("access-keys", "Send SSH keys"),
                    Step::new("access-verify", "Verify SSH"),
                ],
            ),
            Phase::new(
                "Cache Setup",
                vec![Step::new("cache-start", "Start caching proxies")],
            ),
            Phase::new(
                "OS Setup",
                vec![
                    Step::new("cache-configure", "Configure caches on Helios"),
                    Step::new("os-update", "Package update"),
                    Step::new("os-reboot", "Reboot + reconnect"),
                    Step::new("os-cleanup", "Delete old boot environments"),
                    Step::new("os-packages", "Install packages"),
                    Step::new("os-rust", "Install Rust toolchain"),
                    Step::new("os-swap", "Configure swap"),
                ],
            ),
            Phase::new(
                "Build",
                vec![
                    Step::new("repo-clone", "Clone omicron"),
                    Step::new("repo-configure", "Configure build"),
                    Step::new("build-prereqs-builder", "Builder prerequisites"),
                    Step::new("build-prereqs-runner", "Runner prerequisites"),
                    Step::new("build-fix-perms", "Fix file ownership"),
                    Step::new("build-compile", "Compile omicron-package"),
                    Step::new("build-package", "Package components"),
                    Step::new("build-patch", "Patch propolis"),
                ],
            ),
            Phase::new(
                "Deploy",
                vec![
                    Step::new("deploy-vhw", "Create virtual hardware"),
                    Step::new("deploy-install", "Install omicron"),
                    Step::new("deploy-verify", "Verify deployment"),
                ],
            ),
            Phase::new(
                "Configure",
                vec![
                    Step::new("config-quotas", "Set silo quotas"),
                    Step::new("config-ippool", "Create IP pool"),
                ],
            ),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pipeline_structure() {
        let p = build_deploy_pipeline();
        assert_eq!(p.phases.len(), 8);
        assert_eq!(p.phases[0].name, "Provision VM");
        assert_eq!(p.phases[1].name, "Configure VM");
        assert_eq!(p.phases[2].name, "Configure Access");
        assert_eq!(p.phases[3].name, "Cache Setup");
        assert_eq!(p.phases[4].name, "OS Setup");
        assert_eq!(p.phases[5].name, "Build");
        assert_eq!(p.phases[6].name, "Deploy");
        assert_eq!(p.phases[7].name, "Configure");
        let (done, total) = p.progress();
        assert_eq!(done, 0);
        assert_eq!(total, 28);
    }

    #[test]
    fn test_step_lifecycle() {
        let mut p = build_deploy_pipeline();

        // Start a step
        p.start_step("prov-create");
        assert!(p.started.is_some());
        assert!(matches!(
            p.step_mut("prov-create").unwrap().status,
            StepStatus::Running { .. }
        ));

        // Complete it
        p.complete_step("prov-create");
        assert!(matches!(
            p.step_mut("prov-create").unwrap().status,
            StepStatus::Completed { .. }
        ));

        let (done, _) = p.progress();
        assert_eq!(done, 1);
    }

    #[test]
    fn test_step_failure() {
        let mut p = build_deploy_pipeline();
        p.start_step("build-compile");
        p.fail_step("build-compile", "cargo build failed".to_string());

        assert!(p.has_failure());
        assert!(p.phases[5].has_failure());
        assert_eq!(
            p.step_mut("build-compile").unwrap().detail(),
            Some("cargo build failed")
        );
    }

    #[test]
    fn test_find_step() {
        let p = build_deploy_pipeline();
        assert_eq!(p.find_step("prov-create"), Some((0, 0)));
        assert_eq!(p.find_step("access-verify"), Some((2, 1)));
        assert_eq!(p.find_step("cache-configure"), Some((4, 0)));
        assert_eq!(p.find_step("repo-clone"), Some((5, 0)));
        assert_eq!(p.find_step("build-patch"), Some((5, 7)));
        assert_eq!(p.find_step("deploy-verify"), Some((6, 2)));
        assert_eq!(p.find_step("nonexistent"), None);
    }
}
