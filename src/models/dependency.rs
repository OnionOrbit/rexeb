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

    /// Parse from RPM sense flags (`REQUIREFLAGS`/`CONFLICTFLAGS`/...)
    ///
    /// Only the comparison bits are honored (`LESS = 0x02`, `GREATER = 0x04`,
    /// `EQUAL = 0x08`); script qualifiers and friends are ignored. Returns
    /// `None` when no version comparison is requested.
    pub fn from_rpm_flags(flags: u32) -> Option<Self> {
        match flags & 0x0f {
            0x08 => Some(Self::Eq),
            0x02 => Some(Self::Lt),
            0x04 => Some(Self::Gt),
            0x0a => Some(Self::Le),
            0x0c => Some(Self::Ge),
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
    /// Debian architecture qualifier (e.g. `amd64`, `!amd64`), if any
    pub arch_qualifier: Option<String>,
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
            arch_qualifier: None,
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
            arch_qualifier: None,
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
            if suffix.chars().next().is_some_and(|c| c.is_ascii_digit()) ||
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
            let arch_qualifier = caps
                .get(4)
                .map(|m| m.as_str().trim().to_string())
                .filter(|q| !q.is_empty());

            Ok(Self {
                debian_name: name,
                arch_name: None,
                version_op,
                version,
                alternatives: Vec::new(),
                is_virtual: false,
                confidence: 0.0,
                arch_qualifier,
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

    /// Whether this dependency does not apply to the given Debian architecture
    ///
    /// Handles qualifiers like `[amd64]`, `[amd64 arm64]`, `[!amd64]`,
    /// `[!amd64 !arm64]` and wildcards like `[linux-any]`.
    pub fn is_excluded_on(&self, debian_arch: &str) -> bool {
        let qualifier = match self.arch_qualifier {
            Some(ref q) => q,
            None => return false,
        };
        let tokens: Vec<&str> = qualifier.split_whitespace().collect();
        if tokens.is_empty() {
            return false;
        }
        let has_negative = tokens.iter().any(|t| t.starts_with('!'));
        if has_negative {
            // Excluded when the arch matches any negated entry
            tokens
                .iter()
                .filter_map(|t| t.strip_prefix('!'))
                .any(|a| arch_pattern_matches(a, debian_arch))
        } else {
            // Included only when the arch matches at least one entry
            !tokens
                .iter()
                .any(|a| arch_pattern_matches(a, debian_arch))
        }
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

/// Match an architecture qualifier pattern against a Debian architecture
fn arch_pattern_matches(pattern: &str, arch: &str) -> bool {
    if pattern == arch {
        return true;
    }
    // Wildcards like `linux-any` or `any-amd64`
    if let Some((os, rest)) = pattern.split_once('-') {
        if rest == "any" {
            return debian_arch_os(arch) == os;
        }
        if os == "any" {
            return rest == arch;
        }
    }
    false
}

/// Operating system part of a Debian architecture (for `linux-any` matching)
fn debian_arch_os(arch: &str) -> &str {
    match arch {
        "amd64" | "i386" | "arm64" | "armhf" | "armel" | "ppc64el" | "s390x" | "riscv64"
        | "mipsel" | "mips64el" | "powerpc" | "sparc64" => "linux",
        _ => "unknown",
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

    #[test]
    fn test_parse_arch_qualifier() {
        let dep = Dependency::parse("libfoo (>= 1.0) [amd64]").unwrap();
        assert_eq!(dep.debian_name, "libfoo");
        assert_eq!(dep.arch_qualifier.as_deref(), Some("amd64"));
        assert!(!dep.is_excluded_on("amd64"));
        assert!(dep.is_excluded_on("arm64"));

        let neg = Dependency::parse("libbar [!amd64]").unwrap();
        assert!(neg.is_excluded_on("amd64"));
        assert!(!neg.is_excluded_on("arm64"));

        let plain = Dependency::parse("libbaz").unwrap();
        assert!(plain.arch_qualifier.is_none());
        assert!(!plain.is_excluded_on("amd64"));
    }
}