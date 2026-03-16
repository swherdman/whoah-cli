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
    Skipped,
}

impl StepStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            StepStatus::Completed { .. } | StepStatus::Failed { .. } | StepStatus::Skipped
        )
    }
}

// ── Step ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct Step {
    pub id: &'static str,
    pub name: &'static str,
    pub status: StepStatus,
}

impl Step {
    fn new(id: &'static str, name: &'static str) -> Self {
        Self {
            id,
            name,
            status: StepStatus::Pending,
        }
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

    /// Update the detail text of a running step.
    pub fn update_step_detail(&mut self, id: &str, detail: String) {
        if let Some(step) = self.step_mut(id) {
            if let StepStatus::Running { started, .. } = step.status {
                step.status = StepStatus::Running {
                    started,
                    detail: Some(detail),
                };
            }
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

    /// Mark a step as skipped.
    pub fn skip_step(&mut self, id: &str) {
        if let Some(step) = self.step_mut(id) {
            step.status = StepStatus::Skipped;
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
                    Step::new("prov-create", "Create Proxmox VM"),
                    Step::new("prov-boot", "Boot Helios ISO"),
                    Step::new("prov-install", "Install Helios (serial)"),
                    Step::new("prov-network", "Configure networking"),
                    Step::new("prov-netcat", "Netcat user setup"),
                ],
            ),
            Phase::new(
                "Configure Access",
                vec![
                    Step::new("access-keys", "Send SSH keys"),
                    Step::new("access-verify", "Verify SSH connectivity"),
                ],
            ),
            Phase::new(
                "Build & Deploy",
                vec![
                    Step::new("build-pkg-cache", "Start package cache"),
                    Step::new("build-pkg-update", "OS update + reboot"),
                    Step::new("build-del-be", "Delete old boot environments"),
                    Step::new("build-packages", "Install packages"),
                    Step::new("build-rust", "Install Rust toolchain"),
                    Step::new("build-swap", "Configure swap"),
                    Step::new("build-clone", "Clone omicron"),
                    Step::new("build-prereq-builder", "Install builder prerequisites"),
                    Step::new("build-prereq-runner", "Install runner prerequisites"),
                    Step::new("build-fix-perms", "Fix file ownership"),
                    Step::new("build-config-network", "Configure network IPs"),
                    Step::new("build-config-source", "Apply source overrides"),
                    Step::new("build-config-vdevs", "Configure vdev count"),
                    Step::new("build-compile", "Build omicron-package"),
                    Step::new("build-vhw", "Create virtual hardware"),
                    Step::new("build-install", "Install + wait for zones"),
                    Step::new("build-verify", "Verify DNS + API"),
                    Step::new("build-quotas", "Set silo quotas"),
                    Step::new("build-ippool", "Create IP pool"),
                ],
            ),
            Phase::new(
                "Patches",
                vec![
                    Step::new("patch-clone", "Clone propolis"),
                    Step::new("patch-apply", "Apply patch files"),
                    Step::new("patch-build", "Build propolis-server"),
                    Step::new("patch-repack", "Repack tarball"),
                    Step::new("patch-redeploy", "Uninstall + reinstall"),
                    Step::new("patch-verify", "Verify zones"),
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
        assert_eq!(p.phases.len(), 4);
        assert_eq!(p.phases[0].name, "Provision VM");
        assert_eq!(p.phases[1].name, "Configure Access");
        assert_eq!(p.phases[2].name, "Build & Deploy");
        assert_eq!(p.phases[3].name, "Patches");

        let (done, total) = p.progress();
        assert_eq!(done, 0);
        assert_eq!(total, 32);
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

        // Update detail
        p.update_step_detail("prov-create", "Creating VM 302...".to_string());
        assert_eq!(
            p.step_mut("prov-create").unwrap().detail(),
            Some("Creating VM 302...")
        );

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
        assert!(p.phases[2].has_failure());
        assert_eq!(
            p.step_mut("build-compile").unwrap().detail(),
            Some("cargo build failed")
        );
    }

    #[test]
    fn test_find_step() {
        let p = build_deploy_pipeline();
        assert_eq!(p.find_step("prov-create"), Some((0, 0)));
        assert_eq!(p.find_step("access-verify"), Some((1, 1)));
        assert_eq!(p.find_step("patch-verify"), Some((3, 5)));
        assert_eq!(p.find_step("nonexistent"), None);
    }

    #[test]
    fn test_skip_step() {
        let mut p = build_deploy_pipeline();
        p.skip_step("build-swap");

        let (done, _) = p.progress();
        assert_eq!(done, 1); // skipped counts as terminal
    }
}
