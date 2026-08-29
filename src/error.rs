//! Error types for rexeb

use std::path::PathBuf;
use thiserror::Error;

/// Main error type for rexeb operations
#[derive(Error, Debug)]
pub enum RexebError {
    /// I/O error from filesystem operations
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    /// Error while traversing directories with walkdir
    #[error("Walkdir error: {0}")]
    WalkDir(#[from] walkdir::Error),

    /// Failed to parse a `.deb` package
    #[error("Failed to parse .deb package: {0}")]
    DebParsing(String),

    /// Invalid control file syntax or content
    #[error("Invalid control file: {0}")]
    InvalidControl(String),

    /// Missing required field in a control file
    #[error("Missing required field in control file: {0}")]
    MissingField(String),

    /// Failed to extract an archive
    #[error("Failed to extract archive: {0}")]
    Extraction(String),

    /// Invalid or unsupported package architecture
    #[error("Invalid architecture: {0}")]
    InvalidArchitecture(String),

    /// Dependency resolution failure
    #[error("Dependency resolution failed: {0}")]
    DependencyResolution(String),

    /// Package building failure
    #[error("Package building failed: {0}")]
    PackageBuild(String),

    /// Requested file was not found on disk
    #[error("File not found: {path}")]
    FileNotFound {
        /// Path to the file that was not found
        path: PathBuf,
    },

    /// Unsupported package format
    #[error("Unsupported package format: {0}")]
    UnsupportedFormat(String),

    /// Network request or connection error
    #[error("Network error: {0}")]
    Network(String),

    /// Error returned by the AUR RPC API
    #[error("AUR API error: {0}")]
    AurApi(String),

    /// Error while translating maintainer scripts
    #[error("Script translation error: {0}")]
    ScriptTranslation(String),

    /// Package conflict detected during resolution
    #[error("Conflict detected: {0}")]
    Conflict(String),

    /// Validation error for package metadata or inputs
    #[error("Validation error: {0}")]
    Validation(String),

    /// Configuration loading or parsing error
    #[error("Configuration error: {0}")]
    Config(String),

    /// JSON serialization or deserialization error
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// TOML parsing error
    #[error("TOML error: {0}")]
    Toml(#[from] toml::de::Error),

    /// Regex compilation or execution error
    #[error("Regex error: {0}")]
    Regex(#[from] regex::Error),

    /// HTTP request error
    #[error("HTTP request error: {0}")]
    Http(#[from] reqwest::Error),

    /// Catch-all for other uncategorized errors
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