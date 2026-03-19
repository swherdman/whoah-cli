/// Parse JSON log output from `omicron-package package`.
///
/// The LOG file at `~/omicron/out/LOG` uses slog-rs JSON format:
/// ```json
/// {"msg":"propolis-server: verifying hash","package":"propolis-server"}
/// {"msg":"propolis-server: downloading prebuilt","package":"propolis-server"}
/// ```

/// Parsed event from omicron-package LOG.
#[derive(Debug, Clone, PartialEq)]
pub enum OmicronPkgEvent {
    Verifying { package: String },
    Downloading { package: String },
    Other { package: String, msg: String },
}

/// Try to parse a JSON log line from omicron-package.
pub fn parse_omicron_pkg_line(line: &str) -> Option<OmicronPkgEvent> {
    let trimmed = line.trim();
    if !trimmed.starts_with('{') {
        return None;
    }

    // Simple JSON field extraction without a full JSON parser dependency.
    // The format is consistent: {"msg":"...","package":"..."}
    let msg = extract_json_string(trimmed, "msg")?;
    let package = extract_json_string(trimmed, "package")?;

    if msg.contains("verifying hash") {
        Some(OmicronPkgEvent::Verifying { package })
    } else if msg.contains("downloading prebuilt") {
        Some(OmicronPkgEvent::Downloading { package })
    } else {
        Some(OmicronPkgEvent::Other { package, msg })
    }
}

fn extract_json_string(json: &str, key: &str) -> Option<String> {
    // Find "key":"value" pattern
    let pattern = format!("\"{}\":\"", key);
    let start = json.find(&pattern)? + pattern.len();
    let rest = &json[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

/// Tracks omicron-package packaging progress.
#[derive(Debug, Default)]
pub struct OmicronPkgTracker {
    pub packages_seen: u32,
    pub downloading: u32,
    pub current_package: Option<String>,
}

impl OmicronPkgTracker {
    pub fn update(&mut self, event: &OmicronPkgEvent) {
        match event {
            OmicronPkgEvent::Verifying { package } => {
                self.packages_seen += 1;
                self.current_package = Some(package.clone());
            }
            OmicronPkgEvent::Downloading { package } => {
                self.downloading += 1;
                self.current_package = Some(package.clone());
            }
            OmicronPkgEvent::Other { package, .. } => {
                self.current_package = Some(package.clone());
            }
        }
    }

    pub fn summary(&self) -> String {
        if let Some(ref pkg) = self.current_package {
            if self.downloading > 0 {
                format!("Packaging: {pkg} ({} downloaded)", self.downloading)
            } else {
                format!("Packaging: {pkg} ({} verified)", self.packages_seen)
            }
        } else {
            "Packaging components...".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_verifying() {
        let line = r#"{"msg":"propolis-server: verifying hash","v":0,"name":"slog-rs","level":30,"time":"2026-03-19T09:33:01.201449194Z","hostname":"helios02","pid":3605,"package":"propolis-server"}"#;
        let event = parse_omicron_pkg_line(line).unwrap();
        assert_eq!(event, OmicronPkgEvent::Verifying {
            package: "propolis-server".into(),
        });
    }

    #[test]
    fn test_parse_downloading() {
        let line = r#"{"msg":"propolis-server: downloading prebuilt","v":0,"name":"slog-rs","level":30,"time":"2026-03-19T09:33:02.226746814Z","hostname":"helios02","pid":3605,"package":"propolis-server"}"#;
        let event = parse_omicron_pkg_line(line).unwrap();
        assert_eq!(event, OmicronPkgEvent::Downloading {
            package: "propolis-server".into(),
        });
    }

    #[test]
    fn test_parse_target_line() {
        let line = r#"{"msg":"target[clickhouse-topology=single-node image=standard machine=non-gimlet rack-topology=single-sled switch=softnpu ]: TargetMap({\"clickhouse-topology\": \"single-node\"})","v":0,"name":"slog-rs","level":20,"time":"2026-03-19T09:32:50.465626753Z","hostname":"helios02","pid":3605}"#;
        // No "package" field — returns None
        assert!(parse_omicron_pkg_line(line).is_none());
    }

    #[test]
    fn test_parse_non_json() {
        assert!(parse_omicron_pkg_line("Compiling serde v1.0.0").is_none());
        assert!(parse_omicron_pkg_line("").is_none());
    }

    #[test]
    fn test_tracker() {
        let mut t = OmicronPkgTracker::default();
        t.update(&OmicronPkgEvent::Verifying { package: "nexus".into() });
        assert!(t.summary().contains("nexus"));
        assert!(t.summary().contains("1 verified"));

        t.update(&OmicronPkgEvent::Downloading { package: "cockroachdb".into() });
        assert!(t.summary().contains("cockroachdb"));
        assert!(t.summary().contains("1 downloaded"));
    }
}
