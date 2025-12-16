//! Dependency representation and parsing

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fmt;

use crate::error::{RexebError, Result};

/// Version comparison operators
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum VersionOp {
    /// Equal to (=)
    Eq,
    /// Greater than or equal (>=)
    Ge,
    /// Less than or equal (<=)
    Le,
    /// Greater than (>>)
    Gt,
    /// Less than (<<)
    Lt,
}

impl VersionOp {
    /// Convert Debian version operator to Arch Linux format
    pub fn to_arch_format(&self) -> &'static str {
        match self {
            Self::Eq => "=",
            Self::Ge => ">=",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Lt => "<",
        }
    }

    /// Parse from Debian format
    pub fn from_debian(op: &str) -> Option<Self> {
        match op.trim() {
            "=" => Some(Self::Eq),
            ">=" => Some(Self::Ge),
            "<=" => Some(Self::Le),
            ">>" => Some(Self::Gt),
            "<<" => Some(Self::Lt),
            ">" => Some(Self::Gt),
            "<" => Some(Self::Lt),
            _ => None,
        }
    }
}

impl fmt::Display for VersionOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_arch_format())
    }
}

/// Represents a package dependency
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dependency {
    /// Package name (Debian format)
    pub debian_name: String,
    /// Package name (Arch format, after translation)
    pub arch_name: Option<String>,
    /// Version constraint operator
    pub version_op: Option<VersionOp>,
    /// Version string
    pub version: Option<String>,
    /// Alternative dependencies (OR relationship)
    pub alternatives: Vec<Dependency>,
    /// Whether this is a virtual package
    pub is_virtual: bool,
    /// Confidence score for the mapping (0.0 - 1.0)
    pub confidence: f32,
}

impl Dependency {
    /// Create a new dependency with just a name
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            debian_name: name.into(),
            arch_name: None,
            version_op: None,
            version: None,
            alternatives: Vec::new(),
            is_virtual: false,
            confidence: 0.0,
        }
    }

    /// Create a new dependency with version constraint
    pub fn with_version(name: impl Into<String>, op: VersionOp, version: impl Into<String>) -> Self {
        Self {
            debian_name: name.into(),
            arch_name: None,
            version_op: Some(op),
            version: Some(version.into()),
            alternatives: Vec::new(),
            is_virtual: false,
            confidence: 0.0,
        }
    }

    /// Set the Arch package name
    pub fn set_arch_name(&mut self, name: impl Into<String>, confidence: f32) {
        self.arch_name = Some(name.into());
        self.confidence = confidence;
    }

    /// Get the effective package name (Arch if available, otherwise Debian)
    pub fn effective_name(&self) -> &str {
        self.arch_name.as_deref().unwrap_or(&self.debian_name)
    }

    /// Check if this dependency has been successfully mapped
    pub fn is_mapped(&self) -> bool {
        self.arch_name.is_some()
    }

    /// Format for Arch Linux PKGBUILD
    pub fn to_arch_string(&self) -> String {
        let name = self.effective_name();
        match (&self.version_op, &self.version) {
            (Some(op), Some(ver)) => {
                let normalized_ver = Self::normalize_version_for_arch(ver);
                // If version is empty after normalization, just return the name
                if normalized_ver.is_empty() {
                    name.to_string()
                } else {
                    format!("{}{}{}", name, op, normalized_ver)
                }
            }
            _ => name.to_string(),
        }
    }

    /// Normalize a Debian version string for Arch Linux compatibility
    fn normalize_version_for_arch(version: &str) -> String {
        let mut v = version.trim().to_string();
        
        // Remove Debian epoch (e.g., "2:1.8" -> "1.8")
        if let Some(pos) = v.find(':') {
            v = v[pos + 1..].to_string();
        }
        
        // Remove Debian revision suffix (e.g., "1.8.0-1ubuntu1" -> "1.8.0")
        // Keep the main version, remove after the first hyphen followed by debian/ubuntu identifiers
        if let Some(pos) = v.rfind('-') {
            let suffix = &v[pos + 1..];
            // Check if it looks like a Debian revision
            if suffix.chars().next().map_or(false, |c| c.is_ascii_digit()) ||
               suffix.contains("ubuntu") || suffix.contains("debian") ||
               suffix.contains("build") || suffix.contains("deb")
            {
                v = v[..pos].to_string();
            }
        }
        
        // Remove common Debian-specific suffixes
        for suffix in &["+dfsg", "~dfsg", "+ds", "~ds", "+really", "~really"] {
            if let Some(pos) = v.find(suffix) {
                v = v[..pos].to_string();
            }
        }
        
        // Replace ~ with . (Debian uses ~ for pre-release versions)
        v = v.replace('~', ".");
        
        v
    }

    /// Parse a single Debian dependency string (without alternatives)
    fn parse_single(s: &str) -> Result<Self> {
        lazy_static::lazy_static! {
            static ref DEP_RE: Regex = Regex::new(
                r"^\s*([a-zA-Z0-9][a-zA-Z0-9+._-]*)\s*(?:\(\s*(<<|>>|<=|>=|=|<|>)\s*([^)]+)\s*\))?\s*(?:\[([^\]]+)\])?\s*$"
            ).unwrap();
        }

        let s = s.trim();
        if s.is_empty() {
            return Err(RexebError::InvalidControl("Empty dependency".into()));
        }

        if let Some(caps) = DEP_RE.captures(s) {
            let name = caps.get(1).unwrap().as_str().to_string();
            let version_op = caps.get(2).and_then(|m| VersionOp::from_debian(m.as_str()));
            let version = caps.get(3).map(|m| m.as_str().trim().to_string());

            Ok(Self {
                debian_name: name,
                arch_name: None,
                version_op,
                version,
                alternatives: Vec::new(),
                is_virtual: false,
                confidence: 0.0,
            })
        } else {
            // Fallback: just treat the whole thing as a package name
            Ok(Self::new(s.split_whitespace().next().unwrap_or(s)))
        }
    }

    /// Parse a Debian dependency string (may contain alternatives with |)
    pub fn parse(s: &str) -> Result<Self> {
        let parts: Vec<&str> = s.split('|').collect();
        
        if parts.is_empty() {
            return Err(RexebError::InvalidControl("Empty dependency".into()));
        }

        let mut primary = Self::parse_single(parts[0])?;
        
        for alt in parts.iter().skip(1) {
            if let Ok(alt_dep) = Self::parse_single(alt) {
                primary.alternatives.push(alt_dep);
            }
        }

        Ok(primary)
    }

    /// Parse a comma-separated list of dependencies
    pub fn parse_list(s: &str) -> Result<Vec<Self>> {
        let mut deps = Vec::new();
        
        for part in s.split(',') {
            let part = part.trim();
            if !part.is_empty() {
                deps.push(Self::parse(part)?);
            }
        }

        Ok(deps)
    }
}

impl fmt::Display for Dependency {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_arch_string())?;
        
        if !self.alternatives.is_empty() {
            for alt in &self.alternatives {
                write!(f, " | {}", alt)?;
            }
        }

        Ok(())
    }
}

/// Dependency type classification
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DependencyType {
    /// Required runtime dependency
    Depends,
    /// Pre-installation dependency
    PreDepends,
    /// Recommended packages
    Recommends,
    /// Suggested packages
    Suggests,
    /// Conflicting packages
    Conflicts,
    /// Packages this replaces
    Replaces,
    /// Virtual packages provided
    Provides,
    /// Packages this breaks
    Breaks,
    /// Build-time dependencies
    BuildDepends,
}

impl DependencyType {
    /// Get the Debian control field name
    pub fn debian_field(&self) -> &'static str {
        match self {
            Self::Depends => "Depends",
            Self::PreDepends => "Pre-Depends",
            Self::Recommends => "Recommends",
            Self::Suggests => "Suggests",
            Self::Conflicts => "Conflicts",
            Self::Replaces => "Replaces",
            Self::Provides => "Provides",
            Self::Breaks => "Breaks",
            Self::BuildDepends => "Build-Depends",
        }
    }

    /// Get the PKGBUILD array name
    pub fn pkgbuild_field(&self) -> Option<&'static str> {
        match self {
            Self::Depends | Self::PreDepends => Some("depends"),
            Self::Recommends | Self::Suggests => Some("optdepends"),
            Self::Conflicts | Self::Breaks => Some("conflicts"),
            Self::Replaces => Some("replaces"),
            Self::Provides => Some("provides"),
            Self::BuildDepends => Some("makedepends"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_dep() {
        let dep = Dependency::parse("libc6").unwrap();
        assert_eq!(dep.debian_name, "libc6");
        assert!(dep.version_op.is_none());
        assert!(dep.version.is_none());
    }

    #[test]
    fn test_parse_versioned_dep() {
        let dep = Dependency::parse("libc6 (>= 2.17)").unwrap();
        assert_eq!(dep.debian_name, "libc6");
        assert_eq!(dep.version_op, Some(VersionOp::Ge));
        assert_eq!(dep.version.as_deref(), Some("2.17"));
    }

    #[test]
    fn test_parse_alternatives() {
        let dep = Dependency::parse("python3 | python").unwrap();
        assert_eq!(dep.debian_name, "python3");
        assert_eq!(dep.alternatives.len(), 1);
        assert_eq!(dep.alternatives[0].debian_name, "python");
    }

    #[test]
    fn test_parse_dep_list() {
        let deps = Dependency::parse_list("libc6 (>= 2.17), libssl1.1, zlib1g").unwrap();
        assert_eq!(deps.len(), 3);
        assert_eq!(deps[0].debian_name, "libc6");
        assert_eq!(deps[1].debian_name, "libssl1.1");
        assert_eq!(deps[2].debian_name, "zlib1g");
    }
}