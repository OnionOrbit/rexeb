//! Configuration management for rexeb

use std::path::PathBuf;
use serde::{Deserialize, Serialize};

use crate::error::{RexebError, Result};

/// Application configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// General settings
    #[serde(default)]
    pub general: GeneralConfig,
    
    /// Conversion settings
    #[serde(default)]
    pub conversion: ConversionConfig,
    
    /// Network settings
    #[serde(default)]
    pub network: NetworkConfig,
    
    /// Logging settings
    #[serde(default)]
    pub logging: LoggingConfig,
    
    /// Java-specific settings
    #[serde(default)]
    pub java: JavaConfig,
}

/// General configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    /// Cache directory
    pub cache_dir: Option<PathBuf>,
    /// Data directory
    pub data_dir: Option<PathBuf>,
    /// Default output directory
    pub output_dir: Option<PathBuf>,
    /// Number of parallel jobs
    pub jobs: Option<usize>,
    /// Automatically accept prompts
    pub auto_yes: bool,
}

/// Conversion configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionConfig {
    /// Default output format
    pub default_format: String,
    /// Skip dependency resolution
    pub skip_deps: bool,
    /// Generate PKGBUILD instead of binary
    pub generate_pkgbuild: bool,
    /// Keep temporary files
    pub keep_temp: bool,
    /// Minimum confidence for fuzzy matching
    pub min_match_confidence: f32,
    /// Strip binaries
    pub strip_binaries: bool,
}

/// Network configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    /// HTTP timeout in seconds
    pub timeout: u64,
    /// Use proxy
    pub proxy: Option<String>,
    /// AUR RPC URL
    pub aur_url: String,
    /// URL for remote package mappings database
    pub mappings_url: String,
    /// URL for AUR package metadata snapshot
    pub aur_cache_url: String,
    /// debtap script URL harvested by `update --enlarge`
    /// (pin to a commit for reproducible imports)
    pub debtap_url: String,
    /// Enable offline mode
    pub offline: bool,
}

/// Logging configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    /// Log level
    pub level: String,
    /// Log file path
    pub file: Option<PathBuf>,
    /// Enable colored output
    pub color: bool,
}

/// Java-specific configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JavaConfig {
    /// Strategy for handling Java conflicts (jre, jdk, prefer-jdk, prefer-jre, prompt)
    pub conflict_strategy: String,
    /// Whether to add conflict declarations for Java packages
    pub add_java_conflicts: bool,
    /// Default Java version to use when multiple versions are available
    pub default_version: String,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            cache_dir: None,
            data_dir: None,
            output_dir: None,
            jobs: None,
            auto_yes: false,
        }
    }
}

impl Default for ConversionConfig {
    fn default() -> Self {
        Self {
            default_format: "pkg.tar.zst".to_string(),
            skip_deps: false,
            generate_pkgbuild: false,
            keep_temp: false,
            min_match_confidence: 0.6,
            strip_binaries: true,
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            timeout: 30,
            proxy: None,
            aur_url: "https://aur.archlinux.org/rpc/v5".to_string(),
            mappings_url: "https://raw.githubusercontent.com/OnionOrbit/rexeb/main/db/mappings.json".to_string(),
            aur_cache_url: "https://aur.archlinux.org/packages.gz".to_string(),
            debtap_url: "https://raw.githubusercontent.com/helixarch/debtap/master/debtap".to_string(),
            offline: false,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            file: None,
            color: true,
        }
    }
}

impl Default for JavaConfig {
    fn default() -> Self {
        Self {
            conflict_strategy: "prefer-jdk".to_string(),
            add_java_conflicts: true,
            default_version: "latest".to_string(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            conversion: ConversionConfig::default(),
            network: NetworkConfig::default(),
            logging: LoggingConfig::default(),
            java: JavaConfig::default(),
        }
    }
}

impl Config {
    /// Get the config file path
    ///
    /// Honors the `REXEB_CONFIG` environment variable (also populated from
    /// `--config`) so users can point rexeb at an alternate config file.
    pub fn config_path() -> Result<PathBuf> {
        if let Ok(custom) = std::env::var("REXEB_CONFIG") {
            let custom = custom.trim();
            if !custom.is_empty() {
                return Ok(PathBuf::from(custom));
            }
        }
        let config_dir = dirs::config_dir()
            .ok_or_else(|| RexebError::Config("Could not find config directory".into()))?;
        Ok(config_dir.join("rexeb").join("config.toml"))
    }

    /// Load configuration from a specific file
    pub fn load_from(path: &std::path::Path) -> Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    /// Load configuration from file
    pub fn load() -> Result<Self> {
        let path = Self::config_path()?;
        Self::load_from(&path)
    }

    /// Save configuration to file
    pub fn save(&self) -> Result<()> {
        let path = Self::config_path()?;
        
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)
            .map_err(|e| RexebError::Config(e.to_string()))?;
        std::fs::write(&path, content)?;
        
        Ok(())
    }

    /// Reset configuration to defaults
    pub fn reset() -> Result<()> {
        let config = Self::default();
        config.save()
    }

    /// Initialize configuration file
    pub fn init(force: bool) -> Result<()> {
        let path = Self::config_path()?;
        
        if path.exists() && !force {
            return Err(RexebError::Config(
                "Configuration file already exists. Use --force to overwrite.".into()
            ));
        }

        let config = Self::default();
        config.save()
    }

    /// Get a configuration value by key
    pub fn get(&self, key: &str) -> Option<String> {
        match key {
            "general.cache_dir" => self.general.cache_dir.as_ref().map(|p| p.display().to_string()),
            "general.data_dir" => self.general.data_dir.as_ref().map(|p| p.display().to_string()),
            "general.output_dir" => self.general.output_dir.as_ref().map(|p| p.display().to_string()),
            "general.jobs" => self.general.jobs.map(|j| j.to_string()),
            "general.auto_yes" => Some(self.general.auto_yes.to_string()),
            
            "conversion.default_format" => Some(self.conversion.default_format.clone()),
            "conversion.skip_deps" => Some(self.conversion.skip_deps.to_string()),
            "conversion.generate_pkgbuild" => Some(self.conversion.generate_pkgbuild.to_string()),
            "conversion.keep_temp" => Some(self.conversion.keep_temp.to_string()),
            "conversion.strip_binaries" => Some(self.conversion.strip_binaries.to_string()),
            "conversion.min_match_confidence" => Some(self.conversion.min_match_confidence.to_string()),
            
            "network.timeout" => Some(self.network.timeout.to_string()),
            "network.proxy" => self.network.proxy.clone(),
            "network.aur_url" => Some(self.network.aur_url.clone()),
            "network.mappings_url" => Some(self.network.mappings_url.clone()),
            "network.aur_cache_url" => Some(self.network.aur_cache_url.clone()),
            "network.debtap_url" => Some(self.network.debtap_url.clone()),
            "network.offline" => Some(self.network.offline.to_string()),
            
            "logging.level" => Some(self.logging.level.clone()),
            "logging.file" => self.logging.file.as_ref().map(|p| p.display().to_string()),
            "logging.color" => Some(self.logging.color.to_string()),
            
            "java.conflict_strategy" => Some(self.java.conflict_strategy.clone()),
            "java.add_java_conflicts" => Some(self.java.add_java_conflicts.to_string()),
            "java.default_version" => Some(self.java.default_version.clone()),
            
            _ => None,
        }
    }

    /// Set a configuration value by key
    pub fn set(&mut self, key: &str, value: &str) -> Result<()> {
        match key {
            "general.cache_dir" => {
                self.general.cache_dir = Some(PathBuf::from(value));
            }
            "general.data_dir" => {
                self.general.data_dir = Some(PathBuf::from(value));
            }
            "general.output_dir" => {
                self.general.output_dir = Some(PathBuf::from(value));
            }
            "general.jobs" => {
                let jobs: usize = value.parse().map_err(|_| {
                    RexebError::Config("Invalid number for jobs".into())
                })?;
                if jobs == 0 {
                    return Err(RexebError::Config("jobs must be at least 1".into()));
                }
                self.general.jobs = Some(jobs);
            }
            "general.auto_yes" => {
                self.general.auto_yes = value.parse().map_err(|_| {
                    RexebError::Config("Invalid boolean for auto_yes".into())
                })?;
            }
            
            "conversion.default_format" => {
                if crate::cli::OutputFormat::from_config_str(value).is_none() {
                    return Err(RexebError::Config(format!(
                        "Invalid default_format '{}' (expected pkg.tar.zst, pkg.tar.xz, or pkg.tar.gz)",
                        value
                    )));
                }
                self.conversion.default_format = value.to_string();
            }
            "conversion.skip_deps" => {
                self.conversion.skip_deps = value.parse().map_err(|_| {
                    RexebError::Config("Invalid boolean for skip_deps".into())
                })?;
            }
            "conversion.generate_pkgbuild" => {
                self.conversion.generate_pkgbuild = value.parse().map_err(|_| {
                    RexebError::Config("Invalid boolean for generate_pkgbuild".into())
                })?;
            }
            "conversion.keep_temp" => {
                self.conversion.keep_temp = value.parse().map_err(|_| {
                    RexebError::Config("Invalid boolean for keep_temp".into())
                })?;
            }
            "conversion.strip_binaries" => {
                self.conversion.strip_binaries = value.parse().map_err(|_| {
                    RexebError::Config("Invalid boolean for strip_binaries".into())
                })?;
            }
            "conversion.min_match_confidence" => {
                let v: f32 = value.parse().map_err(|_| {
                    RexebError::Config("Invalid number for min_match_confidence".into())
                })?;
                if !v.is_finite() || v < 0.0 || v > 1.0 {
                    return Err(RexebError::Config(
                        "min_match_confidence must be between 0.0 and 1.0".into(),
                    ));
                }
                self.conversion.min_match_confidence = v;
            }
            
            "network.timeout" => {
                self.network.timeout = value.parse().map_err(|_| {
                    RexebError::Config("Invalid number for timeout".into())
                })?;
            }
            "network.proxy" => {
                self.network.proxy = if value.is_empty() { None } else { Some(value.to_string()) };
            }
            "network.aur_url" => {
                self.network.aur_url = value.to_string();
            }
            "network.mappings_url" => {
                self.network.mappings_url = value.to_string();
            }
            "network.aur_cache_url" => {
                self.network.aur_cache_url = value.to_string();
            }
            "network.debtap_url" => {
                if value.trim().is_empty() {
                    return Err(RexebError::Config("debtap_url must not be empty".into()));
                }
                self.network.debtap_url = value.to_string();
            }
            "network.offline" => {
                self.network.offline = value.parse().map_err(|_| {
                    RexebError::Config("Invalid boolean for offline".into())
                })?;
            }
            
            "logging.level" => {
                self.logging.level = value.to_string();
            }
            "logging.file" => {
                self.logging.file = if value.is_empty() { None } else { Some(PathBuf::from(value)) };
            }
            "logging.color" => {
                self.logging.color = value.parse().map_err(|_| {
                    RexebError::Config("Invalid boolean for color".into())
                })?;
            }
            
            "java.conflict_strategy" => {
                match value {
                    "jre" | "jdk" | "prefer-jdk" | "prefer-jre" | "prompt" => {
                        self.java.conflict_strategy = value.to_string();
                    }
                    _ => {
                        return Err(RexebError::Config(format!(
                            "Invalid conflict_strategy '{}' (expected jre, jdk, prefer-jdk, prefer-jre, or prompt)",
                            value
                        )));
                    }
                }
            }
            "java.add_java_conflicts" => {
                self.java.add_java_conflicts = value.parse().map_err(|_| {
                    RexebError::Config("Invalid boolean for add_java_conflicts".into())
                })?;
            }
            "java.default_version" => {
                self.java.default_version = value.to_string();
            }
            
            _ => {
                return Err(RexebError::Config(format!("Unknown configuration key: {}", key)));
            }
        }
        
        Ok(())
    }

    /// Get the cache directory
    pub fn cache_dir(&self) -> PathBuf {
        self.general.cache_dir.clone().unwrap_or_else(|| {
            dirs::cache_dir()
                .unwrap_or_else(|| PathBuf::from("/tmp"))
                .join("rexeb")
        })
    }

    /// Get the data directory
    pub fn data_dir(&self) -> PathBuf {
        self.general.data_dir.clone().unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("/usr/share"))
                .join("rexeb")
        })
    }

    /// Build an HTTP client honoring the `timeout` and `proxy` settings
    pub fn http_client(&self) -> Result<reqwest::Client> {
        let mut builder = reqwest::Client::builder()
            .user_agent(format!("{}/{}", crate::NAME, crate::VERSION))
            .timeout(std::time::Duration::from_secs(self.network.timeout.max(1)));
        if let Some(ref proxy) = self.network.proxy {
            let proxy = proxy.trim();
            if !proxy.is_empty() {
                let parsed = reqwest::Proxy::all(proxy).map_err(|e| {
                    RexebError::Config(format!("Invalid proxy URL '{}': {}", proxy, e))
                })?;
                builder = builder.proxy(parsed);
            }
        }
        builder
            .build()
            .map_err(|e| RexebError::Network(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = Config::default();
        assert_eq!(config.conversion.default_format, "pkg.tar.zst");
        assert!(!config.conversion.skip_deps);
        assert_eq!(config.network.timeout, 30);
    }

    #[test]
    fn test_get_set() {
        let mut config = Config::default();
        
        config.set("general.auto_yes", "true").unwrap();
        assert_eq!(config.get("general.auto_yes"), Some("true".to_string()));
        
        config.set("network.timeout", "60").unwrap();
        assert_eq!(config.get("network.timeout"), Some("60".to_string()));
    }
}
