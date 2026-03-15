use color_eyre::{eyre::eyre, Result};
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ZpoolInfo {
    pub name: String,
    pub size_bytes: u64,
    pub allocated_bytes: u64,
    pub free_bytes: u64,
    pub fragmentation_pct: Option<u8>,
    pub capacity_pct: u8,
    pub health: String,
}

/// Parse output of `zpool list -Hp` (tab-delimited, no headers).
/// Columns: NAME SIZE ALLOC FREE CKPOINT EXPANDSZ FRAG CAP DEDUP HEALTH ALTROOT
pub fn parse_zpool_list(output: &str) -> Result<Vec<ZpoolInfo>> {
    let mut pools = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() < 10 {
            continue;
        }
        let frag = fields[6].trim_end_matches('%');
        let fragmentation_pct = if frag == "-" {
            None
        } else {
            frag.parse().ok()
        };

        pools.push(ZpoolInfo {
            name: fields[0].to_string(),
            size_bytes: fields[1].parse().map_err(|_| eyre!("bad size: {}", fields[1]))?,
            allocated_bytes: fields[2].parse().map_err(|_| eyre!("bad alloc: {}", fields[2]))?,
            free_bytes: fields[3].parse().map_err(|_| eyre!("bad free: {}", fields[3]))?,
            fragmentation_pct,
            capacity_pct: fields[7].parse().map_err(|_| eyre!("bad cap: {}", fields[7]))?,
            health: fields[9].to_string(),
        });
    }
    Ok(pools)
}

/// Filter to just rpool.
pub fn parse_rpool(output: &str) -> Result<Option<ZpoolInfo>> {
    let pools = parse_zpool_list(output)?;
    Ok(pools.into_iter().find(|p| p.name == "rpool"))
}

/// Filter to oxp_ pools only.
pub fn parse_oxp_pools(output: &str) -> Result<Vec<ZpoolInfo>> {
    let pools = parse_zpool_list(output)?;
    Ok(pools.into_iter().filter(|p| p.name.starts_with("oxp_")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_OUTPUT: &str = "\
rpool\t267544698880\t82530148352\t185014550528\t-\t-\t25\t30\t1.00\tONLINE\t-
oxp_abc123\t42949672960\t16267415552\t26682257408\t-\t-\t10\t38\t1.00\tONLINE\t-
oxp_def456\t42949672960\t13314398208\t29635274752\t-\t-\t8\t31\t1.00\tONLINE\t-
oxp_ghi789\t42949672960\t12470607872\t30479065088\t-\t-\t7\t29\t1.00\tONLINE\t-";

    #[test]
    fn test_parse_zpool_list() {
        let pools = parse_zpool_list(SAMPLE_OUTPUT).unwrap();
        assert_eq!(pools.len(), 4);
        assert_eq!(pools[0].name, "rpool");
        assert_eq!(pools[0].capacity_pct, 30);
        assert_eq!(pools[0].health, "ONLINE");
    }

    #[test]
    fn test_parse_rpool() {
        let rpool = parse_rpool(SAMPLE_OUTPUT).unwrap().unwrap();
        assert_eq!(rpool.name, "rpool");
        assert_eq!(rpool.size_bytes, 267544698880);
    }

    #[test]
    fn test_parse_oxp_pools() {
        let oxp = parse_oxp_pools(SAMPLE_OUTPUT).unwrap();
        assert_eq!(oxp.len(), 3);
        assert!(oxp.iter().all(|p| p.name.starts_with("oxp_")));
        assert_eq!(oxp[0].capacity_pct, 38);
    }

    #[test]
    fn test_empty_output() {
        let pools = parse_zpool_list("").unwrap();
        assert!(pools.is_empty());
    }
}
