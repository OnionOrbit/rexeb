//! Architecture conversion and handling

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::{RexebError, Result};

/// Represents the target architecture
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Architecture {
    /// x86_64 / amd64
    X86_64,
    /// i686 / i386
    I686,
    /// aarch64 / arm64
    Aarch64,
    /// armv7h / armhf
    Armv7h,
    /// Any architecture
    Any,
}

impl Architecture {
    /// Convert from Debian architecture name to Arch Linux architecture
    pub fn from_debian(arch: &str) -> Result<Self> {
        match arch.to_lowercase().as_str() {
            "amd64" | "x86_64" => Ok(Self::X86_64),
            "i386" | "i686" => Ok(Self::I686),
            "arm64" | "aarch64" => Ok(Self::Aarch64),
            "armhf" | "armv7l" => Ok(Self::Armv7h),
            "all" | "any" => Ok(Self::Any),
            _ => Err(RexebError::InvalidArchitecture(format!(
                "Unknown Debian architecture: {}",
                arch
            ))),
        }
    }

    /// Get the Arch Linux architecture name
    pub fn to_arch_name(&self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::I686 => "i686",
            Self::Aarch64 => "aarch64",
            Self::Armv7h => "armv7h",
            Self::Any => "any",
        }
    }

    /// Get the Debian architecture name
    pub fn to_debian_name(&self) -> &'static str {
        match self {
            Self::X86_64 => "amd64",
            Self::I686 => "i386",
            Self::Aarch64 => "arm64",
            Self::Armv7h => "armhf",
            Self::Any => "all",
        }
    }

    /// Check if this is a 64-bit architecture
    pub fn is_64bit(&self) -> bool {
        matches!(self, Self::X86_64 | Self::Aarch64)
    }

    /// Get the current system architecture
    pub fn current() -> Self {
        #[cfg(target_arch = "x86_64")]
        return Self::X86_64;
        #[cfg(target_arch = "x86")]
        return Self::I686;
        #[cfg(target_arch = "aarch64")]
        return Self::Aarch64;
        #[cfg(target_arch = "arm")]
        return Self::Armv7h;
        #[cfg(not(any(
            target_arch = "x86_64",
            target_arch = "x86",
            target_arch = "aarch64",
            target_arch = "arm"
        )))]
        return Self::Any;
    }
}

impl fmt::Display for Architecture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_arch_name())
    }
}

impl FromStr for Architecture {
    type Err = RexebError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "x86_64" | "amd64" => Ok(Self::X86_64),
            "i686" | "i386" => Ok(Self::I686),
            "aarch64" | "arm64" => Ok(Self::Aarch64),
            "armv7h" | "armhf" => Ok(Self::Armv7h),
            "any" | "all" => Ok(Self::Any),
            _ => Err(RexebError::InvalidArchitecture(format!(
                "Unknown architecture: {}",
                s
            ))),
        }
    }
}

impl Default for Architecture {
    fn default() -> Self {
        Self::current()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debian_to_arch() {
        assert_eq!(
            Architecture::from_debian("amd64").unwrap(),
            Architecture::X86_64
        );
        assert_eq!(
            Architecture::from_debian("i386").unwrap(),
            Architecture::I686
        );
        assert_eq!(
            Architecture::from_debian("arm64").unwrap(),
            Architecture::Aarch64
        );
        assert_eq!(
            Architecture::from_debian("all").unwrap(),
            Architecture::Any
        );
    }

    #[test]
    fn test_arch_names() {
        assert_eq!(Architecture::X86_64.to_arch_name(), "x86_64");
        assert_eq!(Architecture::I686.to_arch_name(), "i686");
        assert_eq!(Architecture::Aarch64.to_arch_name(), "aarch64");
        assert_eq!(Architecture::Armv7h.to_arch_name(), "armv7h");
        assert_eq!(Architecture::Any.to_arch_name(), "any");
    }
}