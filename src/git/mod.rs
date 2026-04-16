mod provider_github;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

pub use provider_github::GitHubProvider;

// --- Data types (source-agnostic) ---

#[derive(Debug, Clone)]
pub struct RefEntry {
    pub name: String,
    pub sha: String,
}

#[derive(Debug, Clone)]
pub struct CommitEntry {
    pub sha: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct RepoRefs {
    pub default_branch: String,
    pub branches: Vec<RefEntry>,
    pub tags: Vec<RefEntry>,
    pub commits: Vec<CommitEntry>,
    pub fetched_at: Instant,
}

// --- Provider trait ---

/// A source that can fetch git ref metadata for a repository.
/// Implementations: GitHubProvider (gh CLI → HTTP API fallback).
/// Future: GitLabProvider, generic git (ls-remote) fallback.
pub trait GitProvider: Send + Sync {
    /// Whether this provider can handle the given repo URL.
    fn supports(&self, repo_url: &str) -> bool;

    /// Fetch branches, tags, recent commits, and default branch.
    fn fetch_refs(&self, repo_url: &str) -> Result<RepoRefs, String>;
}

// --- Provider resolution ---

/// Returns the appropriate provider for a repo URL.
/// Tries providers in order: GitHub, then future providers.
pub fn provider_for(repo_url: &str) -> Option<Box<dyn GitProvider>> {
    let github = GitHubProvider;
    if github.supports(repo_url) {
        return Some(Box::new(github));
    }
    // Future: GitLabProvider, BitbucketProvider, generic git fallback
    None
}

/// Convenience: fetch refs using the appropriate provider.
pub fn fetch_repo_refs(repo_url: &str) -> Result<RepoRefs, String> {
    let provider = provider_for(repo_url)
        .ok_or_else(|| format!("No git provider supports URL: {repo_url}"))?;
    provider.fetch_refs(repo_url)
}

// --- Session cache (source-agnostic) ---

/// Session-level cache of repo refs, keyed by repo URL.
pub struct RefCache {
    inner: Mutex<HashMap<String, RepoRefs>>,
}

impl Default for RefCache {
    fn default() -> Self {
        Self::new()
    }
}

impl RefCache {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    pub fn get(&self, repo_url: &str) -> Option<RepoRefs> {
        #[allow(clippy::unwrap_used)] // Mutex poison only occurs after a panic elsewhere
        let map = self.inner.lock().unwrap();
        let entry = map.get(repo_url)?;
        if entry.fetched_at.elapsed().as_secs() < 300 {
            Some(entry.clone())
        } else {
            None
        }
    }

    pub fn insert(&self, repo_url: String, refs: RepoRefs) {
        #[allow(clippy::unwrap_used)] // Mutex poison only occurs after a panic elsewhere
        let mut map = self.inner.lock().unwrap();
        map.insert(repo_url, refs);
    }
}
