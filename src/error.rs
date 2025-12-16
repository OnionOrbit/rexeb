//! Error types for rexeb

use std::path::PathBuf;
use thiserror::Error;

/// Main error type for rexeb operations
#[derive(Error, Debug)]
pub enum RexebError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Walkdir error: {0}")]
    WalkDir(#[from] walkdir::Error),

    #[error("Failed to parse .deb package: {0}")]
    DebParsing(String),

    #[error("Invalid control file: {0}")]
    InvalidControl(String),

    #[error("Missing required field in control file: {0}")]
    MissingField(String),

    #[error("Failed to extract archive: {0}")]
    Extraction(String),

    #[error("Invalid architecture: {0}")]
    InvalidArchitecture(String),

    #[error("Dependency resolution failed: {0}")]
    DependencyResolution(String),

    #[error("Package building failed: {0}")]
    PackageBuild(String),

    #[error("File not found: {path}")]
    FileNotFound { path: PathBuf },

    #[error("Unsupported package format: {0}")]
    UnsupportedFormat(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("AUR API error: {0}")]
    AurApi(String),

    #[error("Script translation error: {0}")]
    ScriptTranslation(String),

    #[error("Conflict detected: {0}")]
    Conflict(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),

    #[error("Regex error: {0}")]
    Regex(#[from] regex::Error),

    #[error("HTTP request error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("{0}")]
    Other(String),
}

/// Result type alias for rexeb operations
pub type Result<T> = std::result::Result<T, RexebError>;

impl RexebError {
    /// Create a new parsing error
    pub fn parse(msg: impl Into<String>) -> Self {
        Self::DebParsing(msg.into())
    }

    /// Create a new extraction error
    pub fn extract(msg: impl Into<String>) -> Self {
        Self::Extraction(msg.into())
    }

    /// Create a new dependency resolution error
    pub fn dependency(msg: impl Into<String>) -> Self {
        Self::DependencyResolution(msg.into())
    }

    /// Create a file not found error
    pub fn file_not_found(path: impl Into<PathBuf>) -> Self {
        Self::FileNotFound { path: path.into() }
    }
}