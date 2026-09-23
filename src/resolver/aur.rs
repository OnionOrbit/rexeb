//! AUR (Arch User Repository) integration

use serde::{Deserialize, Serialize};
use reqwest::Client;
use crate::error::{RexebError, Result};

/// AUR RPC endpoint
const AUR_RPC_URL: &str = "https://aur.archlinux.org/rpc/v5";

/// AUR package info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AurPackage {
    /// Package name
    #[serde(rename = "Name")]
    pub name: String,
    /// Package version string
    #[serde(rename = "Version")]
    pub version: String,
    /// Package description, if available
    #[serde(rename = "Description")]
    pub description: Option<String>,
    /// Upstream project URL, if available
    #[serde(rename = "URL")]
    pub url: Option<String>,
    /// Base package name for split packages
    #[serde(rename = "PackageBase")]
    pub package_base: String,
    /// Number of votes on AUR
    #[serde(rename = "NumVotes")]
    pub num_votes: u32,
    /// Popularity score as reported by AUR
    #[serde(rename = "Popularity")]
    pub popularity: f64,
    /// Unix timestamp when flagged out-of-date, if any
    #[serde(rename = "OutOfDate")]
    pub out_of_date: Option<i64>,
    /// Maintainer username, if any
    #[serde(rename = "Maintainer")]
    pub maintainer: Option<String>,
    /// Unix timestamp when first submitted to AUR
    #[serde(rename = "FirstSubmitted")]
    pub first_submitted: i64,
    /// Unix timestamp of last modification
    #[serde(rename = "LastModified")]
    pub last_modified: i64,
    /// Virtual packages or capabilities this package provides
    #[serde(rename = "Provides")]
    pub provides: Option<Vec<String>>,
    /// Packages this package replaces
    #[serde(rename = "Replaces")]
    pub replaces: Option<Vec<String>>,
    /// Packages this package conflicts with
    #[serde(rename = "Conflicts")]
    pub conflicts: Option<Vec<String>>,
}

/// AUR RPC response
#[derive(Debug, Deserialize)]
struct AurResponse {
    /// Number of results reported by the API
    #[serde(rename = "resultcount")]
    result_count: usize,
    /// List of packages returned by the API
    #[serde(rename = "results")]
    results: Vec<AurPackage>,
    /// Response type string, e.g. `"search"` or `"multiinfo"`
    #[serde(rename = "type")]
    response_type: Option<String>,
    /// Error message if the request failed
    #[serde(rename = "error")]
    error: Option<String>,
}

impl AurResponse {
    /// Validate that `result_count` matches the actual number of deserialized results
    /// and log the response type for diagnostics.
    ///
    /// Returns `true` if the count is consistent.
    fn validate(&self) -> bool {
        if let Some(ty) = &self.response_type {
            tracing::debug!("AUR response type: {}", ty);
        }
        if self.result_count != self.results.len() {
            tracing::warn!(
                "AUR resultcount mismatch: header says {} but got {} results",
                self.result_count,
                self.results.len()
            );
            return false;
        }
        true
    }
}

/// Client for interacting with AUR
pub struct AurClient {
    client: Client,
    base_url: String,
}

/// Normalize a configured AUR RPC base URL to the v5 path-style endpoint
///
/// Older configs store `https://aur.archlinux.org/rpc`; the search/info
/// calls below need `.../rpc/v5`.
fn normalize_aur_url(url: &str) -> String {
    let url = url.trim_end_matches('/');
    if url.ends_with("/rpc") {
        format!("{}/v5", url)
    } else {
        url.to_string()
    }
}

impl AurClient {
    /// Create a new AUR client
    ///
    /// Honors `network.timeout`, `network.proxy` and `network.aur_url` from
    /// the user configuration so requests can neither hang forever nor
    /// bypass a configured proxy.
    pub fn new() -> Self {
        let config = crate::config::Config::load().unwrap_or_default();
        let client = config.http_client().unwrap_or_else(|_| {
            Client::builder()
                .user_agent(format!("{}/{}", crate::NAME, crate::VERSION))
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .unwrap_or_default()
        });
        let configured = config.network.aur_url.trim().to_string();
        let base = if configured.is_empty() {
            AUR_RPC_URL
        } else {
            configured.as_str()
        };
        Self {
            client,
            base_url: normalize_aur_url(base),
        }
    }

    /// Create a new AUR client with an explicit base URL (primarily for tests)
    pub fn with_base_url(base_url: &str) -> Self {
        let mut client = Self::new();
        client.base_url = normalize_aur_url(base_url);
        client
    }

    /// Search for packages by name (keyword search)
    pub async fn search(&self, query: &str) -> Result<Vec<AurPackage>> {
        let base = self.base_url.trim_end_matches('/');
        let mut url = reqwest::Url::parse(&format!("{}/search/", base))
            .map_err(|e| RexebError::Network(format!("Invalid AUR URL: {}", e)))?;
        url.path_segments_mut()
            .map_err(|_| RexebError::Network("Invalid AUR URL".into()))?
            .push(query);
        url.query_pairs_mut().append_pair("by", "name-desc");
        self.make_request(url.as_str()).await
    }

    /// Get info for specific packages
    pub async fn info(&self, names: &[&str]) -> Result<Vec<AurPackage>> {
        if names.is_empty() {
            return Ok(Vec::new());
        }

        let base = self.base_url.trim_end_matches('/');
        let mut url = reqwest::Url::parse(&format!("{}/info", base))
            .map_err(|e| RexebError::Network(format!("Invalid AUR URL: {}", e)))?;
        {
            let mut pairs = url.query_pairs_mut();
            for name in names {
                pairs.append_pair("arg[]", name);
            }
        }

        self.make_request(url.as_str()).await
    }

    /// Helper to make requests and parse response
    async fn make_request(&self, url: &str) -> Result<Vec<AurPackage>> {
        if crate::config::Config::load()
            .map(|c| c.network.offline)
            .unwrap_or(false)
        {
            return Err(RexebError::Network("offline mode: AUR request skipped".into()));
        }
        let resp = self.client.get(url)
            .send()
            .await
            .map_err(|e| RexebError::Network(e.to_string()))?;

        if !resp.status().is_success() {
            return Err(RexebError::Network(format!("AUR API error: {}", resp.status())));
        }

        let aur_resp: AurResponse = resp.json()
            .await
            .map_err(|e| RexebError::Network(e.to_string()))?;

        if let Some(err) = aur_resp.error {
            return Err(RexebError::AurApi(err));
        }

        // Use result_count and response_type fields via validation and tracing
        aur_resp.validate();
        tracing::debug!(
            "AUR request to {} returned result_count={} (actual {} results)",
            url,
            aur_resp.result_count,
            aur_resp.results.len()
        );

        Ok(aur_resp.results)
    }

    /// Find packages that provide a specific capability (e.g., a library or virtual package)
    /// Note: AUR RPC v5 doesn't support direct provider search efficiently, 
    /// so this is a best-effort search using keywords
    pub async fn find_providers(&self, capability: &str) -> Result<Vec<AurPackage>> {
        // Search for the capability name
        let mut results = self.search(capability).await?;
        
        // Filter to prioritize packages where name matches or provides contains the capability
        // This filtering happens client-side since RPC search is broad
        results.retain(|pkg| {
            pkg.name == capability || 
            pkg.provides.as_ref().map_or(false, |p| p.iter().any(|prov| prov == capability))
        });
        
        // Sort by popularity
        results.sort_by(|a, b| b.popularity.partial_cmp(&a.popularity).unwrap_or(std::cmp::Ordering::Equal));
        
        Ok(results)
    }
}

impl Default for AurClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_aur_search() {
        let client = AurClient::new();
        // Search for a known package (e.g., yay)
        let results = client.search("yay").await;
        
        // Depending on network/AUR availability this might fail, so we just check Result type
        // In a real test we might want to mock the HTTP client
        if let Ok(pkgs) = results {
            assert!(!pkgs.is_empty());
            assert!(pkgs.iter().any(|p| p.name == "yay"));
        }
    }
}