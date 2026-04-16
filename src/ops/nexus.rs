#![allow(dead_code)]

use color_eyre::{Result, eyre::eyre};
use serde::Serialize;

use crate::config::types::NexusConfig;
use crate::ssh::RemoteHost;

// --- NexusClient ---

pub struct NexusClient<'a> {
    host: &'a dyn RemoteHost,
    nexus_ip: String,
    silo_name: String,
    username: String,
    password: String,
    token: Option<String>,
}

impl<'a> NexusClient<'a> {
    pub fn new(host: &'a dyn RemoteHost, nexus_ip: &str, config: &NexusConfig) -> Self {
        Self {
            host,
            nexus_ip: nexus_ip.to_string(),
            silo_name: config.silo_name.clone(),
            username: config.username.clone(),
            password: config.password.clone(),
            token: None,
        }
    }

    /// Authenticate and cache token. Clears existing token first.
    async fn authenticate(&mut self) -> Result<()> {
        self.token = None;

        let cmd = format!(
            "curl -sf -X POST http://{}/v1/login/{}/local \
             -H 'Content-Type: application/json' \
             -d '{{\"username\":\"{}\",\"password\":\"{}\"}}' \
             -D - 2>/dev/null",
            self.nexus_ip, self.silo_name, self.username, self.password
        );

        let output = self.host.execute(&cmd).await?;

        // Parse session token from set-cookie header
        for line in output.stdout.lines() {
            if let Some(cookie_part) = line.strip_prefix("set-cookie: session=")
                && let Some(token) = cookie_part.split(';').next()
            {
                self.token = Some(token.to_string());
                tracing::debug!("Nexus auth successful");
                return Ok(());
            }
            // Also handle lowercase
            if let Some(cookie_part) = line.strip_prefix("Set-Cookie: session=")
                && let Some(token) = cookie_part.split(';').next()
            {
                self.token = Some(token.to_string());
                tracing::debug!("Nexus auth successful");
                return Ok(());
            }
        }

        Err(eyre!(
            "Nexus auth failed: no session cookie in response (exit {})",
            output.exit_code
        ))
    }

    /// Ensure we have a valid token, authenticating if needed.
    async fn ensure_auth(&mut self) -> Result<()> {
        if self.token.is_none() {
            self.authenticate().await?;
        }
        Ok(())
    }

    /// GET request with auto-auth and 401 retry.
    pub async fn get(&mut self, path: &str) -> Result<ApiResponse> {
        self.request_with_retry("GET", path, None).await
    }

    /// PUT request with auto-auth and 401 retry.
    pub async fn put(&mut self, path: &str, body: &str) -> Result<ApiResponse> {
        self.request_with_retry("PUT", path, Some(body)).await
    }

    /// POST request with auto-auth and 401 retry.
    pub async fn post(&mut self, path: &str, body: &str) -> Result<ApiResponse> {
        self.request_with_retry("POST", path, Some(body)).await
    }

    /// Consolidated retry logic: ensure auth, execute, re-auth on 401, retry once.
    async fn request_with_retry(
        &mut self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<ApiResponse> {
        self.ensure_auth().await?;
        let result = self.do_request(method, path, body).await?;

        if result.status == 401 {
            tracing::debug!("Got 401, re-authenticating...");
            self.authenticate().await?;
            self.do_request(method, path, body).await
        } else {
            Ok(result)
        }
    }

    async fn do_request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
    ) -> Result<ApiResponse> {
        let token = self
            .token
            .as_ref()
            .ok_or_else(|| eyre!("No auth token available"))?;

        let body_args = match body {
            Some(b) => format!("-d '{b}' -H 'Content-Type: application/json'"),
            None => String::new(),
        };

        let cmd = format!(
            "curl -s -o /dev/stdout -w '\\n%{{http_code}}' \
             -X {method} \
             -H 'Cookie: session={token}' \
             {body_args} \
             http://{}/{path} 2>/dev/null",
            self.nexus_ip
        );

        let output = self.host.execute(&cmd).await?;

        // Last line is the HTTP status code (from -w '%{http_code}')
        let lines: Vec<&str> = output.stdout.trim().lines().collect();
        let (body_str, status) = if lines.len() >= 2 {
            let status_str = lines.last().unwrap_or(&"0");
            let body = lines[..lines.len() - 1].join("\n");
            (body, status_str.parse().unwrap_or(0))
        } else if lines.len() == 1 {
            // Might be just the status code with no body
            let s = lines[0].parse().unwrap_or(0);
            if s > 0 {
                (String::new(), s)
            } else {
                (lines[0].to_string(), 0)
            }
        } else {
            (String::new(), 0)
        };

        Ok(ApiResponse {
            status,
            body: body_str,
        })
    }
}

#[derive(Debug)]
pub struct ApiResponse {
    pub status: u16,
    pub body: String,
}

impl ApiResponse {
    pub fn is_ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

// --- Errors and status enums ---

/// Operational errors during checks (SSH down, auth failed, bad response).
/// These are NOT drift states — they mean we couldn't determine the state.
#[derive(Debug, Clone, Serialize)]
pub enum CheckError {
    AuthFailed(String),
    Unreachable,
    ParseError(String),
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AuthFailed(e) => write!(f, "auth failed: {e}"),
            Self::Unreachable => write!(f, "unreachable"),
            Self::ParseError(e) => write!(f, "parse error: {e}"),
        }
    }
}

/// Drift states for silo quotas — what IS the current state relative to desired.
#[derive(Debug, Clone, Serialize)]
pub enum QuotaStatus {
    Ok {
        cpus: u64,
        memory: u64,
        storage: u64,
    },
    ZeroQuotas,
    Mismatch {
        current_cpus: u64,
        current_memory: u64,
        current_storage: u64,
        expected_cpus: u64,
        expected_memory: u64,
        expected_storage: u64,
    },
}

/// Drift states for IP pool — does it exist or not.
#[derive(Debug, Clone, Serialize)]
pub enum IpPoolStatus {
    Ok { name: String },
    Missing,
}

// --- Check functions (read-only) ---

/// Check silo quota values against desired config.
pub async fn check_quotas(
    client: &mut NexusClient<'_>,
    config: &NexusConfig,
) -> std::result::Result<QuotaStatus, CheckError> {
    let path = format!("v1/system/silos/{}/quotas", config.silo_name);
    let response = client
        .get(&path)
        .await
        .map_err(|e| CheckError::AuthFailed(e.to_string()))?;

    if !response.is_ok() {
        if response.status == 401 || response.status == 403 {
            return Err(CheckError::AuthFailed(format!("HTTP {}", response.status)));
        }
        return Err(CheckError::Unreachable);
    }

    let json: serde_json::Value =
        serde_json::from_str(&response.body).map_err(|e| CheckError::ParseError(e.to_string()))?;

    let cpus = json["cpus"].as_u64().unwrap_or(0);
    let memory = json["memory"].as_u64().unwrap_or(0);
    let storage = json["storage"].as_u64().unwrap_or(0);

    if cpus == 0 && memory == 0 && storage == 0 {
        return Ok(QuotaStatus::ZeroQuotas);
    }

    if cpus != config.quotas.cpus
        || memory != config.quotas.memory
        || storage != config.quotas.storage
    {
        return Ok(QuotaStatus::Mismatch {
            current_cpus: cpus,
            current_memory: memory,
            current_storage: storage,
            expected_cpus: config.quotas.cpus,
            expected_memory: config.quotas.memory,
            expected_storage: config.quotas.storage,
        });
    }

    Ok(QuotaStatus::Ok {
        cpus,
        memory,
        storage,
    })
}

/// Check if the expected IP pool exists.
pub async fn check_ip_pool(
    client: &mut NexusClient<'_>,
    config: &NexusConfig,
) -> std::result::Result<IpPoolStatus, CheckError> {
    let response = client
        .get("v1/ip-pools?limit=50")
        .await
        .map_err(|e| CheckError::AuthFailed(e.to_string()))?;

    if !response.is_ok() {
        if response.status == 401 || response.status == 403 {
            return Err(CheckError::AuthFailed(format!("HTTP {}", response.status)));
        }
        return Err(CheckError::Unreachable);
    }

    let json: serde_json::Value =
        serde_json::from_str(&response.body).map_err(|e| CheckError::ParseError(e.to_string()))?;

    if let Some(pools) = json["items"].as_array() {
        for pool in pools {
            if pool["name"].as_str() == Some(&config.ip_pool_name) {
                return Ok(IpPoolStatus::Ok {
                    name: config.ip_pool_name.clone(),
                });
            }
        }
    }

    Ok(IpPoolStatus::Missing)
}

// --- Change functions (apply desired state) ---

/// Set silo quotas to the values from config.
pub async fn set_quotas(client: &mut NexusClient<'_>, config: &NexusConfig) -> Result<()> {
    let path = format!("v1/system/silos/{}/quotas", config.silo_name);
    let body = format!(
        "{{\"cpus\":{},\"memory\":{},\"storage\":{}}}",
        config.quotas.cpus, config.quotas.memory, config.quotas.storage
    );

    let response = client.put(&path, &body).await?;
    if !response.is_ok() {
        return Err(eyre!(
            "Failed to set quotas (HTTP {}): {}",
            response.status,
            response.body
        ));
    }

    tracing::info!(
        "Set silo quotas: cpus={}, memory={}, storage={}",
        config.quotas.cpus,
        config.quotas.memory,
        config.quotas.storage
    );
    Ok(())
}

/// Create the IP pool, link to silo, and add the IP range from config.
pub async fn create_ip_pool(
    client: &mut NexusClient<'_>,
    config: &NexusConfig,
    pool_range_first: &str,
    pool_range_last: &str,
) -> Result<()> {
    // Step 1: Create pool
    let body = format!(
        "{{\"name\":\"{}\",\"description\":\"Default IP pool for instances\"}}",
        config.ip_pool_name
    );
    let response = client.post("v1/ip-pools", &body).await?;
    if !response.is_ok() && response.status != 409 {
        // 409 = already exists, which is fine (idempotent)
        return Err(eyre!(
            "Failed to create IP pool (HTTP {}): {}",
            response.status,
            response.body
        ));
    }

    // Step 2: Link pool to silo as default
    let link_path = format!("v1/system/ip-pools/{}/silos", config.ip_pool_name);
    let link_body = format!("{{\"silo\":\"{}\",\"is_default\":true}}", config.silo_name);
    let response = client.post(&link_path, &link_body).await?;
    if !response.is_ok() && response.status != 409 {
        return Err(eyre!(
            "Failed to link IP pool to silo (HTTP {}): {}",
            response.status,
            response.body
        ));
    }

    // Step 3: Add IP range
    let range_path = format!("v1/ip-pools/{}/ranges/add", config.ip_pool_name);
    let range_body = format!("{{\"first\":\"{pool_range_first}\",\"last\":\"{pool_range_last}\"}}");
    let response = client.post(&range_path, &range_body).await?;
    if !response.is_ok() && response.status != 409 {
        return Err(eyre!(
            "Failed to add IP range (HTTP {}): {}",
            response.status,
            response.body
        ));
    }

    tracing::info!(
        "Created IP pool '{}' with range {}-{}",
        config.ip_pool_name,
        pool_range_first,
        pool_range_last
    );
    Ok(())
}
