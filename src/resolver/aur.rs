//! AUR (Arch User Repository) integration

use serde::{Deserialize, Serialize};
use reqwest::Client;
use crate::error::{RexebError, Result};

/// AUR RPC endpoint
const AUR_RPC_URL: &str = "https://aur.archlinux.org/rpc/v5";

/// AUR package info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AurPackage {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Version")]
    pub version: String,
    #[serde(rename = "Description")]
    pub description: Option<String>,
    #[serde(rename = "URL")]
    pub url: Option<String>,
    #[serde(rename = "PackageBase")]
    pub package_base: String,
    #[serde(rename = "NumVotes")]
    pub num_votes: u32,
    #[serde(rename = "Popularity")]
    pub popularity: f64,
    #[serde(rename = "OutOfDate")]
    pub out_of_date: Option<i64>,
    #[serde(rename = "Maintainer")]
    pub maintainer: Option<String>,
    #[serde(rename = "FirstSubmitted")]
    pub first_submitted: i64,
    #[serde(rename = "LastModified")]
    pub last_modified: i64,
    #[serde(rename = "Provides")]
    pub provides: Option<Vec<String>>,
    #[serde(rename = "Replaces")]
    pub replaces: Option<Vec<String>>,
    #[serde(rename = "Conflicts")]
    pub conflicts: Option<Vec<String>>,
}

/// AUR RPC response
#[derive(Debug, Deserialize)]
struct AurResponse {
    #[serde(rename = "resultcount")]
    result_count: usize,
    #[serde(rename = "results")]
    results: Vec<AurPackage>,
    #[serde(rename = "type")]
    response_type: Option<String>,
    #[serde(rename = "error")]
    error: Option<String>,
}

/// Client for interacting with AUR
pub struct AurClient {
    client: Client,
    base_url: String,
}

impl AurClient {
    /// Create a new AUR client
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .user_agent(format!("{}/{}", crate::NAME, crate::VERSION))
                .build()
                .unwrap_or_default(),
            base_url: AUR_RPC_URL.to_string(),
        }
    }

    /// Search for packages by name (keyword search)
    pub async fn search(&self, query: &str) -> Result<Vec<AurPackage>> {
        let url = format!("{}/search/{}", self.base_url, query);
        self.make_request(&url).await
    }

    /// Get info for specific packages
    pub async fn info(&self, names: &[&str]) -> Result<Vec<AurPackage>> {
        if names.is_empty() {
            return Ok(Vec::new());
        }

        let mut url = format!("{}/info", self.base_url);
        
        // Build query string manually to handle multiple 'arg[]' parameters
        let params: Vec<String> = names.iter()
            .map(|n| format!("arg[]={}", n))
            .collect();
        
        if !params.is_empty() {
            url.push('?');
            url.push_str(&params.join("&"));
        }

        self.make_request(&url).await
    }

    /// Helper to make requests and parse response
    async fn make_request(&self, url: &str) -> Result<Vec<AurPackage>> {
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