//! Parse `cargo xtask download` structured log output.
//!
//! Output format (slog-style):
//! ```text
//! Mar 18 06:17:14.090 INFO Starting download, target: Cockroach
//! Mar 18 06:17:14.093 INFO Downloading out/downloads/cockroach.tgz (attempt 1/3), target: Cockroach
//! Mar 18 06:24:01.849 INFO Unpacking out/downloads/mgd.tar.gz to out/downloads, target: MaghemiteMgd
//! Mar 18 06:24:04.483 INFO Download complete, target: MaghemiteMgd
//! Mar 18 09:35:57.852 INFO Already downloaded (out/downloads/cockroach.tgz), target: Cockroach
//! ```

/// Parsed event from xtask download.
#[derive(Debug, Clone, PartialEq)]
pub enum XtaskEvent {
    Starting {
        target: String,
    },
    Downloading {
        target: String,
        file: Option<String>,
    },
    Unpacking {
        target: String,
    },
    Complete {
        target: String,
    },
    Cached {
        target: String,
    },
}

/// Try to parse a line of xtask download output.
pub fn parse_xtask_line(line: &str) -> Option<XtaskEvent> {
    // All relevant lines contain "INFO" and "target:"
    if !line.contains("INFO") {
        return None;
    }

    let target = extract_target(line)?;

    if line.contains("Starting download") {
        return Some(XtaskEvent::Starting { target });
    }

    if line.contains("Already downloaded") {
        return Some(XtaskEvent::Cached { target });
    }

    if line.contains("Download complete") {
        return Some(XtaskEvent::Complete { target });
    }

    if line.contains("Unpacking") {
        return Some(XtaskEvent::Unpacking { target });
    }

    if line.contains("Downloading") {
        let file = line
            .split("Downloading ")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .map(|s| s.to_string());
        return Some(XtaskEvent::Downloading { target, file });
    }

    None
}

fn extract_target(line: &str) -> Option<String> {
    // "..., target: Cockroach"
    line.split("target: ").nth(1).map(|s| s.trim().to_string())
}

/// Tracks xtask download progress.
#[derive(Debug, Default)]
pub struct XtaskTracker {
    pub total_targets: u32,
    pub completed: u32,
    pub cached: u32,
    pub current_target: Option<String>,
}

impl XtaskTracker {
    pub fn update(&mut self, event: &XtaskEvent) {
        match event {
            XtaskEvent::Starting { target } => {
                self.total_targets += 1;
                self.current_target = Some(target.clone());
            }
            XtaskEvent::Complete { .. } => {
                self.completed += 1;
            }
            XtaskEvent::Cached { .. } => {
                self.completed += 1;
                self.cached += 1;
            }
            XtaskEvent::Downloading { target, .. } => {
                self.current_target = Some(target.clone());
            }
            XtaskEvent::Unpacking { target } => {
                self.current_target = Some(target.clone());
            }
        }
    }

    pub fn summary(&self) -> String {
        if self.total_targets == 0 {
            return "Starting downloads...".to_string();
        }
        if self.completed >= self.total_targets {
            if self.cached == self.total_targets {
                return format!("All {} tools cached", self.total_targets);
            }
            return format!("All {} tools downloaded", self.total_targets);
        }
        if let Some(ref target) = self.current_target {
            format!(
                "Downloading {target} ({}/{})",
                self.completed, self.total_targets
            )
        } else {
            format!("{}/{} tools", self.completed, self.total_targets)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_starting() {
        let line = "Mar 18 06:17:14.090 INFO Starting download, target: Cockroach";
        assert_eq!(
            parse_xtask_line(line),
            Some(XtaskEvent::Starting {
                target: "Cockroach".into()
            })
        );
    }

    #[test]
    fn test_parse_downloading() {
        let line = "Mar 18 06:17:14.093 INFO Downloading out/downloads/cockroach.tgz (attempt 1/3), target: Cockroach";
        let event = parse_xtask_line(line).unwrap();
        match event {
            XtaskEvent::Downloading { target, file } => {
                assert_eq!(target, "Cockroach");
                assert_eq!(file, Some("out/downloads/cockroach.tgz".into()));
            }
            other => panic!("Expected Downloading, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_complete() {
        let line = "Mar 18 06:24:04.483 INFO Download complete, target: MaghemiteMgd";
        assert_eq!(
            parse_xtask_line(line),
            Some(XtaskEvent::Complete {
                target: "MaghemiteMgd".into()
            })
        );
    }

    #[test]
    fn test_parse_cached() {
        let line = "Mar 18 09:35:57.852 INFO Already downloaded (out/downloads/cockroach.tgz), target: Cockroach";
        assert_eq!(
            parse_xtask_line(line),
            Some(XtaskEvent::Cached {
                target: "Cockroach".into()
            })
        );
    }

    #[test]
    fn test_parse_unpacking() {
        let line = "Mar 18 06:24:01.849 INFO Unpacking out/downloads/mgd.tar.gz to out/downloads, target: MaghemiteMgd";
        assert_eq!(
            parse_xtask_line(line),
            Some(XtaskEvent::Unpacking {
                target: "MaghemiteMgd".into()
            })
        );
    }

    #[test]
    fn test_parse_irrelevant() {
        assert!(parse_xtask_line("Compiling serde v1.0.0").is_none());
        assert!(parse_xtask_line("").is_none());
    }

    #[test]
    fn test_tracker() {
        let mut t = XtaskTracker::default();

        t.update(&XtaskEvent::Starting {
            target: "Cockroach".into(),
        });
        t.update(&XtaskEvent::Starting {
            target: "Clickhouse".into(),
        });
        assert_eq!(t.total_targets, 2);

        t.update(&XtaskEvent::Complete {
            target: "Cockroach".into(),
        });
        assert_eq!(t.summary(), "Downloading Clickhouse (1/2)");

        t.update(&XtaskEvent::Complete {
            target: "Clickhouse".into(),
        });
        assert_eq!(t.summary(), "All 2 tools downloaded");
    }

    #[test]
    fn test_tracker_all_cached() {
        let mut t = XtaskTracker::default();
        t.update(&XtaskEvent::Starting { target: "A".into() });
        t.update(&XtaskEvent::Starting { target: "B".into() });
        t.update(&XtaskEvent::Cached { target: "A".into() });
        t.update(&XtaskEvent::Cached { target: "B".into() });
        assert_eq!(t.summary(), "All 2 tools cached");
    }
}
