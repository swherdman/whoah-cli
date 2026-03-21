use std::time::Instant;

use super::{CommitEntry, GitProvider, RefEntry, RepoRefs};

/// GitHub provider — uses `gh` CLI for authenticated API access.
/// Falls back to unauthenticated curl if gh is not installed (60 req/hr limit).
pub struct GitHubProvider;

impl GitProvider for GitHubProvider {
    fn supports(&self, repo_url: &str) -> bool {
        repo_url.contains("github.com")
    }

    fn fetch_refs(&self, repo_url: &str) -> Result<RepoRefs, String> {
        let (owner, repo) = parse_owner_repo(repo_url)
            .ok_or_else(|| format!("Cannot parse GitHub owner/repo from: {repo_url}"))?;

        let api = pick_api_method()?;

        let default_branch = api(&format!("repos/{owner}/{repo}"))
            .and_then(|json| {
                extract_string_field(&json, "default_branch")
                    .ok_or_else(|| "No default_branch in response".to_string())
            })
            .unwrap_or_else(|_| "main".to_string());

        let branches = api(&format!(
            "repos/{owner}/{repo}/branches?per_page=30&sort=updated"
        ))
        .map(|json| parse_ref_list(&json))
        .unwrap_or_default();

        let tags = api(&format!("repos/{owner}/{repo}/tags?per_page=30"))
            .map(|json| parse_ref_list(&json))
            .unwrap_or_default();

        let commits = api(&format!(
            "repos/{owner}/{repo}/commits?per_page=10&sha={default_branch}"
        ))
        .map(|json| parse_commit_list(&json))
        .unwrap_or_default();

        Ok(RepoRefs {
            default_branch,
            branches,
            tags,
            commits,
            fetched_at: Instant::now(),
        })
    }
}

// --- API method selection ---

type ApiFn = Box<dyn Fn(&str) -> Result<String, String>>;

/// Pick the best available API method: gh CLI (authenticated) or curl (unauthenticated).
fn pick_api_method() -> Result<ApiFn, String> {
    if has_command("gh") {
        return Ok(Box::new(gh_api));
    }
    if has_command("curl") {
        return Ok(Box::new(curl_api));
    }
    Err("Neither gh CLI nor curl found. Install gh from https://cli.github.com/".to_string())
}

fn has_command(cmd: &str) -> bool {
    std::process::Command::new(cmd)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Authenticated GitHub API via gh CLI (5000 req/hr).
fn gh_api(endpoint: &str) -> Result<String, String> {
    let output = std::process::Command::new("gh")
        .args(["api", endpoint])
        .output()
        .map_err(|e| format!("Failed to run gh: {e}"))?;

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(format!("gh api {endpoint} failed: {err}"));
    }

    String::from_utf8(output.stdout).map_err(|e| format!("Invalid UTF-8: {e}"))
}

/// Unauthenticated GitHub API via curl (60 req/hr).
fn curl_api(endpoint: &str) -> Result<String, String> {
    let url = format!("https://api.github.com/{endpoint}");
    let output = std::process::Command::new("curl")
        .args(["-sf", "-H", "Accept: application/vnd.github+json", &url])
        .output()
        .map_err(|e| format!("Failed to run curl: {e}"))?;

    if !output.status.success() {
        return Err(format!("curl {url} failed (HTTP error or rate limited)"));
    }

    String::from_utf8(output.stdout).map_err(|e| format!("Invalid UTF-8: {e}"))
}

// --- URL parsing ---

/// Extract (owner, repo) from a GitHub URL.
fn parse_owner_repo(url: &str) -> Option<(String, String)> {
    let path = url.strip_prefix("https://github.com/")?;
    let path = path.strip_suffix(".git").unwrap_or(path);
    let mut parts = path.splitn(2, '/');
    let owner = parts.next()?.to_string();
    let repo = parts.next()?.to_string();
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    Some((owner, repo))
}

// --- JSON parsing ---

fn parse_ref_list(json: &str) -> Vec<RefEntry> {
    let mut refs = Vec::new();
    let mut pos = 0;

    while pos < json.len() {
        if let Some(name) = find_json_string(json, &mut pos, "\"name\"") {
            let sha = find_json_string(json, &mut pos, "\"sha\"").unwrap_or_default();
            if !name.is_empty() {
                refs.push(RefEntry { name, sha });
            }
        } else {
            break;
        }
    }

    refs
}

fn parse_commit_list(json: &str) -> Vec<CommitEntry> {
    let mut commits = Vec::new();
    let mut pos = 0;

    while pos < json.len() {
        if let Some(sha) = find_json_string(json, &mut pos, "\"sha\"") {
            let message = find_json_string(json, &mut pos, "\"message\"")
                .unwrap_or_default();
            let first_line = message.lines().next().unwrap_or("").to_string();
            if !sha.is_empty() {
                commits.push(CommitEntry {
                    sha,
                    message: first_line,
                });
            }
        } else {
            break;
        }
    }

    commits
}

fn find_json_string(json: &str, pos: &mut usize, key: &str) -> Option<String> {
    let idx = json[*pos..].find(key)?;
    let after_key = *pos + idx + key.len();
    let colon = json[after_key..].find('"')?;
    let start = after_key + colon + 1;
    let end_quote = json[start..].find('"')?;
    let value = &json[start..start + end_quote];
    *pos = start + end_quote + 1;
    Some(value.to_string())
}

fn extract_string_field(json: &str, field: &str) -> Option<String> {
    let key = format!("\"{field}\"");
    let mut pos = 0;
    find_json_string(json, &mut pos, &key)
}
