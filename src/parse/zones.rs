use std::collections::HashMap;

use color_eyre::Result;
use serde::Serialize;

/// Known control plane service prefixes, ordered longest-first
/// so multi-word names like "crucible_pantry" match before "crucible".
const KNOWN_SERVICES: &[&str] = &[
    "crucible_pantry",
    "propolis-server",
    "internal_dns",
    "external_dns",
    "cockroachdb",
    "clickhouse",
    "oximeter",
    "crucible",
    "nexus",
    "ntp",
];

/// Infrastructure zone names that don't have the oxz_ prefix.
const INFRA_ZONES_RAW: &[&str] = &["sidecar_softnpu"];

/// Infrastructure service names (after stripping oxz_ prefix).
const INFRA_SERVICES: &[&str] = &["switch"];

#[derive(Debug, Clone, Serialize)]
pub struct ZoneInfo {
    pub id: Option<u32>,
    pub name: String,
    pub status: ZoneStatus,
    pub path: String,
    pub uuid: String,
    pub brand: String,
    pub ip_type: String,
    pub kind: ZoneKind,
    pub service_name: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ZoneStatus {
    Running,
    Installed,
    Configured,
    Incomplete,
    Ready,
    Unknown(String),
}

impl From<&str> for ZoneStatus {
    fn from(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "installed" => Self::Installed,
            "configured" => Self::Configured,
            "incomplete" => Self::Incomplete,
            "ready" => Self::Ready,
            other => Self::Unknown(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ZoneKind {
    /// Control plane service (nexus, crdb, dns, crucible, etc.)
    Service,
    /// VM instance (propolis-server)
    Instance,
    /// Infrastructure (sidecar_softnpu, switch)
    Infrastructure,
}

/// Parse output of `zoneadm list -cp` (colon-delimited).
/// Each line: id:name:state:path:uuid:brand:ip-type
pub fn parse_zoneadm_list(output: &str) -> Result<Vec<ZoneInfo>> {
    let mut zones = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.splitn(7, ':').collect();
        if fields.len() < 7 {
            continue;
        }
        let id = if fields[0] == "-" {
            None
        } else {
            fields[0].parse().ok()
        };
        let name = fields[1];
        // Skip the global zone
        if name == "global" {
            continue;
        }

        let (kind, service_name) = classify_zone(name);

        zones.push(ZoneInfo {
            id,
            name: name.to_string(),
            status: ZoneStatus::from(fields[2]),
            path: fields[3].to_string(),
            uuid: fields[4].to_string(),
            brand: fields[5].to_string(),
            ip_type: fields[6].to_string(),
            kind,
            service_name,
        });
    }
    Ok(zones)
}

/// Classify a zone by name and extract its service name.
fn classify_zone(name: &str) -> (ZoneKind, String) {
    // Check raw infrastructure zones (no oxz_ prefix)
    for &infra in INFRA_ZONES_RAW {
        if name == infra {
            return (ZoneKind::Infrastructure, name.to_string());
        }
    }

    // Strip oxz_ prefix for service matching
    let stripped = match name.strip_prefix("oxz_") {
        Some(s) => s,
        None => return (ZoneKind::Infrastructure, name.to_string()),
    };

    // Check infrastructure services
    for &infra in INFRA_SERVICES {
        if stripped == infra || stripped.starts_with(&format!("{infra}_")) {
            return (ZoneKind::Infrastructure, infra.to_string());
        }
    }

    // Match against known service prefixes (longest first)
    for &svc in KNOWN_SERVICES {
        if stripped == svc || stripped.starts_with(&format!("{svc}_")) {
            let kind = if svc == "propolis-server" {
                ZoneKind::Instance
            } else {
                ZoneKind::Service
            };
            return (kind, svc.to_string());
        }
    }

    // Unknown oxz_ zone — treat as service with the full stripped name
    (ZoneKind::Service, stripped.to_string())
}

/// Derive zone-to-zpool placement from zone paths.
/// Includes Service and Instance zones (not infrastructure).
/// Returns: pool_name -> list of service names
pub fn derive_zone_placement(zones: &[ZoneInfo]) -> HashMap<String, Vec<String>> {
    let mut placement: HashMap<String, Vec<String>> = HashMap::new();
    for zone in zones {
        if zone.kind == ZoneKind::Infrastructure {
            continue;
        }
        if let Some(pool_name) = extract_pool_from_path(&zone.path) {
            placement
                .entry(pool_name)
                .or_default()
                .push(zone.service_name.clone());
        }
    }
    placement
}

/// Extract the pool identifier from a zone path.
/// Zone paths use the format: /pool/ext/<UUID>/crypt/zone/...
/// zpool names are oxp_<UUID>, so we prepend oxp_ to the UUID.
fn extract_pool_from_path(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').collect();
    // Look for /pool/ext/<UUID>/crypt pattern
    for i in 0..parts.len().saturating_sub(1) {
        if parts[i] == "ext" {
            let uuid = parts.get(i + 1)?;
            // Verify it looks like a UUID (contains hyphens, >8 chars)
            if uuid.len() > 8 && uuid.contains('-') {
                return Some(format!("oxp_{uuid}"));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real-world data from the Helios server
    // Zone paths use bare UUIDs: /pool/ext/<UUID>/crypt/zone/...
    // zpool names are oxp_<UUID>
    const REAL_OUTPUT: &str = "\
0:global:running:/::ipkg:shared
780:sidecar_softnpu:running:/sidecar/sidecar_softnpu:aaa:omicron1:excl
781:oxz_switch:running:/zone/oxz_switch:bbb:omicron1:excl
784:oxz_internal_dns_35486e95:running:/pool/ext/af5cfbf7-0f55-4c04/crypt/zone/oxz_internal_dns_35486e95:ccc:omicron1:excl
786:oxz_cockroachdb_22bb5c45:running:/pool/ext/7003baec-20d6-4267/crypt/zone/oxz_cockroachdb_22bb5c45:ddd:omicron1:excl
789:oxz_external_dns_7f2ab990:running:/pool/ext/ae0ac0ee-146c-4212/crypt/zone/oxz_external_dns_7f2ab990:eee:omicron1:excl
790:oxz_crucible_pantry_dbacdf6a:running:/pool/ext/7003baec-20d6-4267/crypt/zone/oxz_crucible_pantry_dbacdf6a:fff:omicron1:excl
793:oxz_crucible_66009ec5:running:/pool/ext/7003baec-20d6-4267/crypt/zone/oxz_crucible_66009ec5:ggg:omicron1:excl
799:oxz_nexus_5eb902a5:running:/pool/ext/af5cfbf7-0f55-4c04/crypt/zone/oxz_nexus_5eb902a5:hhh:omicron1:excl
804:oxz_propolis-server_9be5fc93:running:/pool/ext/af5cfbf7-0f55-4c04/crypt/zone/oxz_propolis-server_9be5fc93:iii:omicron1:excl";

    #[test]
    fn test_classify_infrastructure() {
        let (kind, name) = classify_zone("sidecar_softnpu");
        assert_eq!(kind, ZoneKind::Infrastructure);
        assert_eq!(name, "sidecar_softnpu");

        let (kind, name) = classify_zone("oxz_switch");
        assert_eq!(kind, ZoneKind::Infrastructure);
        assert_eq!(name, "switch");
    }

    #[test]
    fn test_classify_services() {
        let (kind, name) = classify_zone("oxz_cockroachdb_22bb5c45");
        assert_eq!(kind, ZoneKind::Service);
        assert_eq!(name, "cockroachdb");

        let (kind, name) = classify_zone("oxz_nexus_5eb902a5");
        assert_eq!(kind, ZoneKind::Service);
        assert_eq!(name, "nexus");

        let (kind, name) = classify_zone("oxz_internal_dns_35486e95");
        assert_eq!(kind, ZoneKind::Service);
        assert_eq!(name, "internal_dns");
    }

    #[test]
    fn test_classify_crucible_pantry() {
        // Must match "crucible_pantry" not just "crucible"
        let (kind, name) = classify_zone("oxz_crucible_pantry_dbacdf6a");
        assert_eq!(kind, ZoneKind::Service);
        assert_eq!(name, "crucible_pantry");

        let (kind, name) = classify_zone("oxz_crucible_66009ec5");
        assert_eq!(kind, ZoneKind::Service);
        assert_eq!(name, "crucible");
    }

    #[test]
    fn test_classify_instance() {
        let (kind, name) = classify_zone("oxz_propolis-server_9be5fc93");
        assert_eq!(kind, ZoneKind::Instance);
        assert_eq!(name, "propolis-server");
    }

    #[test]
    fn test_parse_real_data() {
        let zones = parse_zoneadm_list(REAL_OUTPUT).unwrap();
        assert_eq!(zones.len(), 9); // 10 lines minus global

        let services: Vec<_> = zones
            .iter()
            .filter(|z| z.kind == ZoneKind::Service)
            .collect();
        let instances: Vec<_> = zones
            .iter()
            .filter(|z| z.kind == ZoneKind::Instance)
            .collect();
        let infra: Vec<_> = zones
            .iter()
            .filter(|z| z.kind == ZoneKind::Infrastructure)
            .collect();

        assert_eq!(services.len(), 6);
        assert_eq!(instances.len(), 1);
        assert_eq!(infra.len(), 2);
        assert_eq!(instances[0].service_name, "propolis-server");
    }

    #[test]
    fn test_placement_includes_services_and_instances() {
        let zones = parse_zoneadm_list(REAL_OUTPUT).unwrap();
        let placement = derive_zone_placement(&zones);

        // infra zones should not appear in placement
        for (_pool, names) in &placement {
            assert!(!names.contains(&"sidecar_softnpu".to_string()));
            assert!(!names.contains(&"switch".to_string()));
        }

        let all_names: Vec<_> = placement.values().flatten().collect();

        // Services should appear
        assert!(all_names.contains(&&"crucible_pantry".to_string()));
        assert!(all_names.contains(&&"crucible".to_string()));

        // Instances should also appear in placement
        assert!(all_names.contains(&&"propolis-server".to_string()));
    }

    #[test]
    fn test_empty_output() {
        let zones = parse_zoneadm_list("").unwrap();
        assert!(zones.is_empty());
    }
}
