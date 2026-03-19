/// Parse `dladm show-ether -p -o LINK` output to extract the NIC device name.
/// Output is one link per line in parseable format, e.g.:
///   e1000g0
pub fn parse_ether_link(output: &str) -> Option<String> {
    output
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .map(|l| l.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_link() {
        assert_eq!(parse_ether_link("e1000g0\n"), Some("e1000g0".to_string()));
    }

    #[test]
    fn test_parse_with_whitespace() {
        assert_eq!(parse_ether_link("  e1000g0  \n"), Some("e1000g0".to_string()));
    }

    #[test]
    fn test_parse_empty() {
        assert_eq!(parse_ether_link(""), None);
        assert_eq!(parse_ether_link("\n"), None);
    }

    #[test]
    fn test_parse_multiple_links() {
        // Takes the first one
        assert_eq!(
            parse_ether_link("igb0\nigb1\n"),
            Some("igb0".to_string())
        );
    }
}
