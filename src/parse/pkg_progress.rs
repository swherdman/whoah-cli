/// Parse `pkg install -v` / `pkg update -v` progress output.
///
/// Key output patterns:
/// ```text
/// Download: 1031/7134 items   83.6/238.8MB  35% complete (16.8M/s)
/// Actions: 1/7672 actions (Installing new actions)
/// No updates necessary for this image.
/// ```

/// Parsed download progress from `pkg install/update -v`.
#[derive(Debug, Clone, PartialEq)]
pub struct PkgDownloadProgress {
    pub items_done: u32,
    pub items_total: u32,
    pub mb_done: f32,
    pub mb_total: f32,
    pub pct: u32,
    pub speed: Option<String>,
}

/// Parsed action progress from `pkg install/update -v`.
#[derive(Debug, Clone, PartialEq)]
pub struct PkgActionProgress {
    pub done: u32,
    pub total: u32,
}

/// Result of parsing a line of pkg output.
#[derive(Debug, Clone, PartialEq)]
pub enum PkgEvent {
    Download(PkgDownloadProgress),
    Actions(PkgActionProgress),
    UpToDate,
    NoChanges,
    Planning(String),
}

/// Try to parse a line of `pkg install/update -v` output.
pub fn parse_pkg_line(line: &str) -> Option<PkgEvent> {
    let trimmed = line.trim();

    if trimmed == "No updates necessary for this image." {
        return Some(PkgEvent::UpToDate);
    }

    if trimmed == "No changes required." {
        return Some(PkgEvent::NoChanges);
    }

    // Planning: Solver setup ... Done (2.429s)
    if trimmed.starts_with("Planning:") {
        return Some(PkgEvent::Planning(trimmed.to_string()));
    }

    // Download: 1031/7134 items   83.6/238.8MB  35% complete (16.8M/s)
    if trimmed.starts_with("Download:") && trimmed.contains("items") {
        return parse_download_line(trimmed);
    }

    // Actions: 1/7672 actions
    if trimmed.starts_with("Actions:") && trimmed.contains("actions") {
        return parse_actions_line(trimmed);
    }

    None
}

fn parse_download_line(line: &str) -> Option<PkgEvent> {
    // "Download: 1031/7134 items   83.6/238.8MB  35% complete (16.8M/s)"
    let after_colon = line.strip_prefix("Download:")?.trim();

    // Parse items: "1031/7134"
    let items_part = after_colon.split_whitespace().next()?;
    let (items_done, items_total) = items_part.split_once('/')?;
    let items_done: u32 = items_done.parse().ok()?;
    let items_total: u32 = items_total.parse().ok()?;

    // Parse MB: "83.6/238.8MB"
    let mb_part = after_colon.split_whitespace().find(|s| s.ends_with("MB"))?;
    let mb_str = mb_part.strip_suffix("MB")?;
    let (mb_done_str, mb_total_str) = mb_str.split_once('/')?;
    let mb_done: f32 = mb_done_str.parse().ok()?;
    let mb_total: f32 = mb_total_str.parse().ok()?;

    // Parse percent: "35%"
    let pct_part = after_colon.split_whitespace().find(|s| s.ends_with('%'))?;
    let pct: u32 = pct_part.strip_suffix('%')?.parse().ok()?;

    // Parse speed (optional): "(16.8M/s)"
    let speed = after_colon
        .split('(')
        .nth(1)
        .and_then(|s| s.strip_suffix(')'))
        .map(|s| s.to_string());

    Some(PkgEvent::Download(PkgDownloadProgress {
        items_done,
        items_total,
        mb_done,
        mb_total,
        pct,
        speed,
    }))
}

fn parse_actions_line(line: &str) -> Option<PkgEvent> {
    // "Actions: 1/7672 actions (Installing new actions)"
    let after_colon = line.strip_prefix("Actions:")?.trim();
    let ratio = after_colon.split_whitespace().next()?;
    let (done, total) = ratio.split_once('/')?;
    let done: u32 = done.parse().ok()?;
    let total: u32 = total.parse().ok()?;
    Some(PkgEvent::Actions(PkgActionProgress { done, total }))
}

/// Format a PkgEvent as a concise summary string.
pub fn format_pkg_event(event: &PkgEvent) -> String {
    match event {
        PkgEvent::Download(p) => {
            if let Some(ref speed) = p.speed {
                format!(
                    "Download: {}/{} items {:.1}/{:.1}MB {}% ({})",
                    p.items_done, p.items_total, p.mb_done, p.mb_total, p.pct, speed
                )
            } else {
                format!(
                    "Download: {}/{} items {:.1}/{:.1}MB {}%",
                    p.items_done, p.items_total, p.mb_done, p.mb_total, p.pct
                )
            }
        }
        PkgEvent::Actions(a) => format!("Actions: {}/{}", a.done, a.total),
        PkgEvent::UpToDate => "Already up to date".to_string(),
        PkgEvent::NoChanges => "No changes required".to_string(),
        PkgEvent::Planning(s) => s.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_download_with_speed() {
        let line = "Download: 1031/7134 items   83.6/238.8MB  35% complete (16.8M/s)";
        let event = parse_pkg_line(line).unwrap();
        assert_eq!(
            event,
            PkgEvent::Download(PkgDownloadProgress {
                items_done: 1031,
                items_total: 7134,
                mb_done: 83.6,
                mb_total: 238.8,
                pct: 35,
                speed: Some("16.8M/s".into()),
            })
        );
    }

    #[test]
    fn test_parse_download_no_speed() {
        let line = "Download:    0/7134 items    0.0/238.8MB  0% complete ";
        let event = parse_pkg_line(line).unwrap();
        assert_eq!(
            event,
            PkgEvent::Download(PkgDownloadProgress {
                items_done: 0,
                items_total: 7134,
                mb_done: 0.0,
                mb_total: 238.8,
                pct: 0,
                speed: None,
            })
        );
    }

    #[test]
    fn test_parse_download_complete() {
        let line = "Download: Completed 238.83 MB in 15.47 seconds (15.4M/s)";
        // This line doesn't match the items pattern, so returns None
        assert!(parse_pkg_line(line).is_none());
    }

    #[test]
    fn test_parse_actions() {
        let line = " Actions:    1/7672 actions (Installing new actions)";
        let event = parse_pkg_line(line).unwrap();
        assert_eq!(
            event,
            PkgEvent::Actions(PkgActionProgress {
                done: 1,
                total: 7672,
            })
        );
    }

    #[test]
    fn test_parse_up_to_date() {
        assert_eq!(
            parse_pkg_line("No updates necessary for this image."),
            Some(PkgEvent::UpToDate),
        );
    }

    #[test]
    fn test_parse_no_changes() {
        assert_eq!(
            parse_pkg_line("No changes required."),
            Some(PkgEvent::NoChanges),
        );
    }

    #[test]
    fn test_parse_planning() {
        let line = "Planning: Solver setup ... Done (2.429s)";
        match parse_pkg_line(line) {
            Some(PkgEvent::Planning(s)) => assert!(s.contains("Solver")),
            other => panic!("Expected Planning, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_irrelevant_line() {
        assert!(parse_pkg_line("helios-dev").is_none());
        assert!(parse_pkg_line("").is_none());
        assert!(parse_pkg_line("  Done").is_none());
    }

    #[test]
    fn test_format_download() {
        let event = PkgEvent::Download(PkgDownloadProgress {
            items_done: 1031,
            items_total: 7134,
            mb_done: 83.6,
            mb_total: 238.8,
            pct: 35,
            speed: Some("16.8M/s".into()),
        });
        let s = format_pkg_event(&event);
        assert!(s.contains("1031/7134"));
        assert!(s.contains("35%"));
        assert!(s.contains("16.8M/s"));
    }
}
