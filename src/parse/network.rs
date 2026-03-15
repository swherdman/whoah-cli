use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DnsResult {
    pub resolved: bool,
    pub addresses: Vec<String>,
}

/// Parse exit code of `curl -sf http://<ip>/v1/ping`.
pub fn parse_nexus_ping(exit_code: i32) -> bool {
    exit_code == 0
}

/// Parse output of `dig recovery.sys.oxide.test @<ip> +short`.
pub fn parse_dns_check(output: &str) -> DnsResult {
    let addresses: Vec<String> = output
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    DnsResult {
        resolved: !addresses.is_empty(),
        addresses,
    }
}

/// Parse whether simnets exist from `dladm show-simnet` output.
/// Exit code 0 with output means simnets exist.
pub fn parse_simnet_check(exit_code: i32, output: &str) -> bool {
    exit_code == 0 && !output.trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nexus_reachable() {
        assert!(parse_nexus_ping(0));
        assert!(!parse_nexus_ping(22));
    }

    #[test]
    fn test_dns_resolving() {
        let result = parse_dns_check("192.168.2.72\n192.168.2.73\n");
        assert!(result.resolved);
        assert_eq!(result.addresses.len(), 2);
    }

    #[test]
    fn test_dns_not_resolving() {
        let result = parse_dns_check("");
        assert!(!result.resolved);
        assert!(result.addresses.is_empty());
    }

    #[test]
    fn test_simnets_exist() {
        assert!(parse_simnet_check(0, "net0\tnet1\n"));
        assert!(!parse_simnet_check(0, ""));
        assert!(!parse_simnet_check(1, ""));
    }
}
