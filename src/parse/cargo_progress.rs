/// Parse cargo build output for progress tracking.
///
/// Key output patterns (without `-v` flag):
/// ```text
///    Compiling serde v1.0.228
///    Downloading crates ...
///    Downloaded serde v1.0.228
///     Finished `release` profile [optimized] target(s) in 5m 38s
/// ```

/// Result of parsing a line of cargo output.
#[derive(Debug, Clone, PartialEq)]
pub enum CargoEvent {
    Compiling {
        crate_name: String,
        version: Option<String>,
    },
    Downloading,
    Downloaded {
        crate_name: String,
    },
    Finished {
        duration: String,
    },
}

/// Try to parse a line of cargo build output.
pub fn parse_cargo_line(line: &str) -> Option<CargoEvent> {
    let trimmed = line.trim();

    if let Some(rest) = trimmed.strip_prefix("Compiling ") {
        let parts: Vec<&str> = rest.splitn(2, ' ').collect();
        let crate_name = parts[0].to_string();
        let version = parts.get(1).map(|v| {
            // Strip version prefix 'v' and any source location
            v.trim_start_matches('v')
                .split(' ')
                .next()
                .unwrap_or(v)
                .to_string()
        });
        return Some(CargoEvent::Compiling {
            crate_name,
            version,
        });
    }

    if trimmed.starts_with("Downloading crates") || trimmed == "Downloading" {
        return Some(CargoEvent::Downloading);
    }

    if let Some(rest) = trimmed.strip_prefix("Downloaded ") {
        let crate_name = rest.split_whitespace().next()?.to_string();
        return Some(CargoEvent::Downloaded { crate_name });
    }

    if let Some(rest) = trimmed.strip_prefix("Finished ") {
        // "Finished `release` profile [optimized] target(s) in 5m 38s"
        if let Some(time_idx) = rest.find(" in ") {
            let duration = rest[time_idx + 4..].to_string();
            return Some(CargoEvent::Finished { duration });
        }
    }

    None
}

/// Tracks cargo build progress across multiple lines.
#[derive(Debug, Default)]
pub struct CargoTracker {
    pub compiled_count: u32,
    pub downloaded_count: u32,
    pub last_crate: Option<String>,
    pub finished: bool,
    pub finish_duration: Option<String>,
    /// Total crate count from cargo tree (for percentage display).
    pub estimated_total: Option<u32>,
}

impl CargoTracker {
    pub fn update(&mut self, event: &CargoEvent) {
        match event {
            CargoEvent::Compiling { crate_name, .. } => {
                self.compiled_count += 1;
                self.last_crate = Some(crate_name.clone());
            }
            CargoEvent::Downloaded { .. } => {
                self.downloaded_count += 1;
            }
            CargoEvent::Finished { duration } => {
                self.finished = true;
                self.finish_duration = Some(duration.clone());
            }
            CargoEvent::Downloading => {}
        }
    }

    pub fn set_estimated_total(&mut self, total: u32) {
        self.estimated_total = Some(total);
    }

    /// Format a summary of the current state.
    pub fn summary(&self) -> String {
        if self.finished {
            if let Some(ref dur) = self.finish_duration {
                return format!("Finished in {dur}");
            }
            return "Finished".to_string();
        }
        if let Some(ref name) = self.last_crate {
            if let Some(total) = self.estimated_total {
                let pct = (self.compiled_count as f32 / total as f32 * 100.0).min(100.0) as u32;
                format!(
                    "Compiling {name} ({}/{total} \u{2014} {pct}%)",
                    self.compiled_count
                )
            } else {
                format!("Compiling {name} ({} crates)", self.compiled_count)
            }
        } else if self.downloaded_count > 0 {
            format!("Downloaded {} crates", self.downloaded_count)
        } else {
            "Starting...".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_compiling() {
        let event = parse_cargo_line("   Compiling serde v1.0.228").unwrap();
        assert_eq!(
            event,
            CargoEvent::Compiling {
                crate_name: "serde".into(),
                version: Some("1.0.228".into()),
            }
        );
    }

    #[test]
    fn test_parse_compiling_path_dep() {
        let event =
            parse_cargo_line("   Compiling omicron-package v0.1.0 (/home/user/omicron/package)")
                .unwrap();
        assert_eq!(
            event,
            CargoEvent::Compiling {
                crate_name: "omicron-package".into(),
                version: Some("0.1.0".into()),
            }
        );
    }

    #[test]
    fn test_parse_compiling_git_dep() {
        let event = parse_cargo_line(
            "   Compiling bhyve_api v0.0.0 (https://github.com/oxidecomputer/propolis?rev=36f20be#36f20be9)"
        ).unwrap();
        assert_eq!(
            event,
            CargoEvent::Compiling {
                crate_name: "bhyve_api".into(),
                version: Some("0.0.0".into()),
            }
        );
    }

    #[test]
    fn test_parse_downloading() {
        assert_eq!(
            parse_cargo_line("  Downloading crates ..."),
            Some(CargoEvent::Downloading)
        );
    }

    #[test]
    fn test_parse_downloaded() {
        let event = parse_cargo_line("   Downloaded serde v1.0.228").unwrap();
        assert_eq!(
            event,
            CargoEvent::Downloaded {
                crate_name: "serde".into()
            }
        );
    }

    #[test]
    fn test_parse_finished() {
        let event =
            parse_cargo_line("    Finished `release` profile [optimized] target(s) in 5m 38s")
                .unwrap();
        assert_eq!(
            event,
            CargoEvent::Finished {
                duration: "5m 38s".into()
            }
        );
    }

    #[test]
    fn test_parse_irrelevant() {
        assert!(parse_cargo_line("     Running `rustc ...`").is_none());
        assert!(parse_cargo_line("warning: unused variable").is_none());
        assert!(parse_cargo_line("").is_none());
    }

    #[test]
    fn test_tracker() {
        let mut t = CargoTracker::default();
        assert_eq!(t.summary(), "Starting...");

        t.update(&CargoEvent::Compiling {
            crate_name: "serde".into(),
            version: None,
        });
        assert_eq!(t.summary(), "Compiling serde (1 crates)");

        t.update(&CargoEvent::Compiling {
            crate_name: "tokio".into(),
            version: None,
        });
        assert_eq!(t.summary(), "Compiling tokio (2 crates)");

        t.update(&CargoEvent::Finished {
            duration: "5m 38s".into(),
        });
        assert_eq!(t.summary(), "Finished in 5m 38s");
    }

    #[test]
    fn test_tracker_with_total() {
        let mut t = CargoTracker::default();
        t.set_estimated_total(583);

        t.update(&CargoEvent::Compiling {
            crate_name: "serde".into(),
            version: None,
        });
        assert_eq!(t.summary(), "Compiling serde (1/583 \u{2014} 0%)");

        for _ in 0..422 {
            t.update(&CargoEvent::Compiling {
                crate_name: "x".into(),
                version: None,
            });
        }
        t.update(&CargoEvent::Compiling {
            crate_name: "tokio".into(),
            version: None,
        });
        assert_eq!(t.summary(), "Compiling tokio (424/583 \u{2014} 72%)");
    }

    #[test]
    fn test_tracker_without_total_unchanged() {
        let mut t = CargoTracker::default();
        t.update(&CargoEvent::Compiling {
            crate_name: "serde".into(),
            version: None,
        });
        assert_eq!(t.summary(), "Compiling serde (1 crates)");
    }
}
