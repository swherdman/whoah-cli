use color_eyre::Result;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct ServiceInfo {
    pub state: ServiceState,
    pub fmri: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ServiceState {
    Online,
    Offline,
    Disabled,
    Maintenance,
    Degraded,
    Uninitialized,
    Unknown(String),
}

impl std::fmt::Display for ServiceState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Online => write!(f, "online"),
            Self::Offline => write!(f, "offline"),
            Self::Disabled => write!(f, "disabled"),
            Self::Maintenance => write!(f, "maintenance"),
            Self::Degraded => write!(f, "degraded"),
            Self::Uninitialized => write!(f, "uninitialized"),
            Self::Unknown(s) => write!(f, "{s}"),
        }
    }
}

impl From<&str> for ServiceState {
    fn from(s: &str) -> Self {
        match s.trim() {
            "online" => Self::Online,
            "online*" => Self::Online, // transitioning
            "offline" => Self::Offline,
            "offline*" => Self::Offline,
            "disabled" => Self::Disabled,
            "maintenance" => Self::Maintenance,
            "degraded" => Self::Degraded,
            "uninitialized" => Self::Uninitialized,
            other => Self::Unknown(other.to_string()),
        }
    }
}

/// Parse output of `svcs -H -o state,fmri <service_names>`.
/// Each line: STATE FMRI (whitespace-separated)
pub fn parse_svcs(output: &str) -> Result<Vec<ServiceInfo>> {
    let mut services = Vec::new();
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let state_str = parts.next().unwrap_or("");
        let fmri = parts.next().unwrap_or("").trim();
        if fmri.is_empty() {
            continue;
        }
        services.push(ServiceInfo {
            state: ServiceState::from(state_str),
            fmri: fmri.to_string(),
        });
    }
    Ok(services)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_svcs() {
        let output = "online         svc:/system/sled-agent:default\n\
                       online         svc:/system/omicron/baseline:default\n";
        let svcs = parse_svcs(output).unwrap();
        assert_eq!(svcs.len(), 2);
        assert_eq!(svcs[0].state, ServiceState::Online);
        assert!(svcs[0].fmri.contains("sled-agent"));
        assert_eq!(svcs[1].state, ServiceState::Online);
    }

    #[test]
    fn test_maintenance_state() {
        let output = "maintenance    svc:/system/sled-agent:default\n";
        let svcs = parse_svcs(output).unwrap();
        assert_eq!(svcs[0].state, ServiceState::Maintenance);
    }

    #[test]
    fn test_transitioning_state() {
        let output = "online*        svc:/system/sled-agent:default\n";
        let svcs = parse_svcs(output).unwrap();
        assert_eq!(svcs[0].state, ServiceState::Online);
    }
}
