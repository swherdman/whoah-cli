use color_eyre::Result;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct VdevFileInfo {
    pub path: String,
    pub size_blocks: u64,
    pub size_bytes: u64,
}

/// Parse output of `ls -s /var/tmp/*.vdev`.
/// Each line: BLOCKS FILENAME (space-separated, blocks are 512 bytes on illumos)
pub fn parse_vdev_files(output: &str) -> Result<Vec<VdevFileInfo>> {
    let mut files = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() || line.contains("No such file") || line.contains("not found") {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let blocks_str = parts.next().unwrap_or("0");
        let path = parts.next().unwrap_or("").trim();
        if path.is_empty() {
            continue;
        }
        let size_blocks: u64 = blocks_str.parse().unwrap_or(0);
        files.push(VdevFileInfo {
            path: path.to_string(),
            size_blocks,
            size_bytes: size_blocks * 512,
        });
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_vdev_files() {
        let output = " 24313856 /var/tmp/u2_0.vdev\n\
                        21102592 /var/tmp/u2_1.vdev\n\
                        21692416 /var/tmp/u2_2.vdev\n";
        let files = parse_vdev_files(output).unwrap();
        assert_eq!(files.len(), 3);
        assert_eq!(files[0].path, "/var/tmp/u2_0.vdev");
        assert_eq!(files[0].size_blocks, 24313856);
        assert_eq!(files[0].size_bytes, 24313856 * 512);
    }

    #[test]
    fn test_no_vdev_files() {
        let output = "ls: /var/tmp/*.vdev: No such file or directory\n";
        let files = parse_vdev_files(output).unwrap();
        assert!(files.is_empty());
    }

    #[test]
    fn test_empty_output() {
        let files = parse_vdev_files("").unwrap();
        assert!(files.is_empty());
    }
}
