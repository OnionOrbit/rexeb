//! Package metadata representation

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use super::{Architecture, Dependency, DependencyType};

/// Source package format
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PackageFormat {
    /// Debian package (.deb)
    Deb,
    /// RPM package (.rpm)
    Rpm,
    /// Alpine package (.apk)
    Apk,
    /// AppImage
    AppImage,
    /// Arch Linux package
    ArchPkg,
}

impl PackageFormat {
    /// Get file extension for this format
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Deb => "deb",
            Self::Rpm => "rpm",
            Self::Apk => "apk",
            Self::AppImage => "AppImage",
            Self::ArchPkg => "pkg.tar.zst",
        }
    }

    /// Detect format from file path
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        match ext.to_lowercase().as_str() {
            "deb" => Some(Self::Deb),
            "rpm" => Some(Self::Rpm),
            "apk" => Some(Self::Apk),
            "appimage" => Some(Self::AppImage),
            "zst" | "xz" | "gz" => {
                // Check for .pkg.tar.* pattern
                let stem = path.file_stem()?.to_str()?;
                if stem.ends_with(".pkg.tar") || stem.ends_with(".pkg") {
                    Some(Self::ArchPkg)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Maintainer script type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MaintainerScript {
    /// Pre-installation script
    PreInst,
    /// Post-installation script
    PostInst,
    /// Pre-removal script
    PreRm,
    /// Post-removal script
    PostRm,
    /// Configuration script (debconf)
    Config,
}

impl MaintainerScript {
    /// Get the Debian script filename
    pub fn debian_name(&self) -> &'static str {
        match self {
            Self::PreInst => "preinst",
            Self::PostInst => "postinst",
            Self::PreRm => "prerm",
            Self::PostRm => "postrm",
            Self::Config => "config",
        }
    }

    /// Get the corresponding .install function name
    pub fn install_function(&self) -> &'static str {
        match self {
            Self::PreInst => "pre_install",
            Self::PostInst => "post_install",
            Self::PreRm => "pre_remove",
            Self::PostRm => "post_remove",
            Self::Config => "post_install", // Config usually runs during post_install
        }
    }

    /// Get the upgrade function name (for when package is being upgraded)
    pub fn upgrade_function(&self) -> &'static str {
        match self {
            Self::PreInst => "pre_upgrade",
            Self::PostInst => "post_upgrade",
            Self::PreRm => "pre_upgrade",
            Self::PostRm => "post_upgrade",
            Self::Config => "post_upgrade",
        }
    }
}

/// Detected license type
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum License {
    /// GPL version 2
    GPL2,
    /// GPL version 3
    GPL3,
    /// LGPL
    LGPL,
    /// MIT license
    MIT,
    /// Apache 2.0
    Apache2,
    /// BSD license
    BSD,
    /// Mozilla Public License
    MPL,
    /// Custom/proprietary license
    Custom(String),
    /// Unknown license
    Unknown,
}

impl License {
    /// Parse license from string
    pub fn from_str(s: &str) -> Self {
        let s_lower = s.to_lowercase();
        
        if s_lower.contains("gpl-3") || s_lower.contains("gplv3") || s_lower.contains("gpl3") {
            Self::GPL3
        } else if s_lower.contains("gpl-2") || s_lower.contains("gplv2") || s_lower.contains("gpl2") {
            Self::GPL2
        } else if s_lower.contains("lgpl") {
            Self::LGPL
        } else if s_lower.contains("mit") {
            Self::MIT
        } else if s_lower.contains("apache") {
            Self::Apache2
        } else if s_lower.contains("bsd") {
            Self::BSD
        } else if s_lower.contains("mpl") || s_lower.contains("mozilla") {
            Self::MPL
        } else if s_lower.contains("proprietary") || s_lower.contains("commercial") {
            Self::Custom(s.to_string())
        } else if s.is_empty() {
            Self::Unknown
        } else {
            Self::Custom(s.to_string())
        }
    }

    /// Get PKGBUILD license string
    pub fn to_pkgbuild(&self) -> String {
        match self {
            Self::GPL2 => "GPL2".to_string(),
            Self::GPL3 => "GPL3".to_string(),
            Self::LGPL => "LGPL".to_string(),
            Self::MIT => "MIT".to_string(),
            Self::Apache2 => "Apache".to_string(),
            Self::BSD => "BSD".to_string(),
            Self::MPL => "MPL".to_string(),
            Self::Custom(s) => format!("custom:{}", s),
            Self::Unknown => "unknown".to_string(),
        }
    }
}

/// Package metadata extracted from source package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMetadata {
    /// Package name (original)
    pub name: String,
    /// Package name (converted for Arch)
    pub arch_name: Option<String>,
    /// Package version
    pub version: String,
    /// Package release/revision number
    pub release: String,
    /// Package epoch
    pub epoch: Option<u32>,
    /// Architecture
    pub arch: Architecture,
    /// Package description
    pub description: String,
    /// Extended description
    pub long_description: Option<String>,
    /// Package URL/homepage
    pub url: Option<String>,
    /// License information
    pub license: License,
    /// Maintainer/packager
    pub maintainer: Option<String>,
    /// Installed size in bytes
    pub installed_size: u64,
    /// Source package format
    pub source_format: PackageFormat,
    /// Section/category
    pub section: Option<String>,
    /// Priority
    pub priority: Option<String>,
    /// Dependencies by type
    pub dependencies: HashMap<DependencyType, Vec<Dependency>>,
    /// Maintainer scripts content
    pub scripts: HashMap<MaintainerScript, String>,
    /// Configuration files
    pub conffiles: Vec<PathBuf>,
    /// Files that will be installed
    pub files: Vec<PathBuf>,
    /// MD5 sums of files (if available)
    pub md5sums: HashMap<PathBuf, String>,
    /// Extra metadata fields
    pub extra: HashMap<String, String>,
}

impl PackageMetadata {
    /// Create a new empty metadata structure
    pub fn new(name: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            arch_name: None,
            version: version.into(),
            release: "1".to_string(),
            epoch: None,
            arch: Architecture::default(),
            description: String::new(),
            long_description: None,
            url: None,
            license: License::Unknown,
            maintainer: None,
            installed_size: 0,
            source_format: PackageFormat::Deb,
            section: None,
            priority: None,
            dependencies: HashMap::new(),
            scripts: HashMap::new(),
            conffiles: Vec::new(),
            files: Vec::new(),
            md5sums: HashMap::new(),
            extra: HashMap::new(),
        }
    }

    /// Get the effective package name (Arch name if set, otherwise original)
    pub fn effective_name(&self) -> &str {
        self.arch_name.as_deref().unwrap_or(&self.name)
    }

    /// Get full version string with epoch if present
    pub fn full_version(&self) -> String {
        match self.epoch {
            Some(e) if e > 0 => format!("{}:{}-{}", e, self.version, self.release),
            _ => format!("{}-{}", self.version, self.release),
        }
    }

    /// Get dependencies of a specific type
    pub fn get_deps(&self, dep_type: DependencyType) -> &[Dependency] {
        self.dependencies.get(&dep_type).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Add a dependency
    pub fn add_dep(&mut self, dep_type: DependencyType, dep: Dependency) {
        self.dependencies.entry(dep_type).or_default().push(dep);
    }

    /// Set maintainer script
    pub fn set_script(&mut self, script_type: MaintainerScript, content: String) {
        self.scripts.insert(script_type, content);
    }

    /// Get maintainer script content
    pub fn get_script(&self, script_type: MaintainerScript) -> Option<&str> {
        self.scripts.get(&script_type).map(|s| s.as_str())
    }

    /// Convert Debian version to Arch-compatible version
    pub fn normalize_version(&mut self) {
        // Remove Debian-specific suffixes and convert to Arch format
        let mut version = self.version.clone();
        
        // Handle epoch
        if let Some(pos) = version.find(':') {
            if let Ok(epoch) = version[..pos].parse::<u32>() {
                self.epoch = Some(epoch);
            }
            version = version[pos + 1..].to_string();
        }

        // Split version and release
        if let Some(pos) = version.rfind('-') {
            self.release = version[pos + 1..].to_string();
            version = version[..pos].to_string();
        }

        // Replace characters not allowed in Arch versions
        version = version.replace('~', ".");
        version = version.replace('+', ".");
        
        // Remove Debian-specific suffixes
        for suffix in &["ubuntu", "build", "deb", "dfsg"] {
            if let Some(pos) = version.to_lowercase().find(suffix) {
                version = version[..pos].trim_end_matches('.').to_string();
            }
        }

        self.version = version;
        
        // Normalize release
        let release = std::mem::take(&mut self.release);
        self.release = release
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if self.release.is_empty() {
            self.release = "1".to_string();
        }
    }

    /// Generate PKGINFO content for .PKGINFO file
    pub fn to_pkginfo(&self) -> String {
        let mut lines = Vec::new();
        
        lines.push(format!("pkgname = {}", self.effective_name()));
        lines.push(format!("pkgver = {}", self.full_version()));
        lines.push(format!("pkgdesc = {}", self.description));
        
        if let Some(ref url) = self.url {
            lines.push(format!("url = {}", url));
        }
        
        lines.push(format!("builddate = {}", chrono::Utc::now().timestamp()));
        
        if let Some(ref maintainer) = self.maintainer {
            lines.push(format!("packager = {}", maintainer));
        }
        
        lines.push(format!("size = {}", self.installed_size));
        lines.push(format!("arch = {}", self.arch.to_arch_name()));
        lines.push(format!("license = {}", self.license.to_pkgbuild()));

        // Dependencies
        for dep in self.get_deps(DependencyType::Depends) {
            lines.push(format!("depend = {}", dep.to_arch_string()));
        }
        for dep in self.get_deps(DependencyType::PreDepends) {
            lines.push(format!("depend = {}", dep.to_arch_string()));
        }
        
        // Optional dependencies
        for dep in self.get_deps(DependencyType::Recommends) {
            lines.push(format!("optdepend = {}", dep.to_arch_string()));
        }
        for dep in self.get_deps(DependencyType::Suggests) {
            lines.push(format!("optdepend = {}", dep.to_arch_string()));
        }

        // Conflicts
        for dep in self.get_deps(DependencyType::Conflicts) {
            lines.push(format!("conflict = {}", dep.to_arch_string()));
        }
        for dep in self.get_deps(DependencyType::Breaks) {
            lines.push(format!("conflict = {}", dep.to_arch_string()));
        }

        // Replaces
        for dep in self.get_deps(DependencyType::Replaces) {
            lines.push(format!("replaces = {}", dep.to_arch_string()));
        }

        // Provides
        for dep in self.get_deps(DependencyType::Provides) {
            lines.push(format!("provides = {}", dep.to_arch_string()));
        }

        lines.join("\n")
    }

    /// Generate PKGBUILD content
    pub fn to_pkgbuild(&self) -> String {
        let mut lines = Vec::new();
        
        lines.push("# Maintainer: Converted by rexeb".to_string());
        if let Some(ref maintainer) = self.maintainer {
            lines.push(format!("# Original: {}", maintainer));
        }
        lines.push(String::new());
        
        lines.push(format!("pkgname={}", self.effective_name()));
        
        if let Some(epoch) = self.epoch {
            if epoch > 0 {
                lines.push(format!("epoch={}", epoch));
            }
        }
        
        lines.push(format!("pkgver={}", self.version));
        lines.push(format!("pkgrel={}", self.release));
        lines.push(format!("pkgdesc=\"{}\"", self.description.replace('"', "\\\"")));
        lines.push(format!("arch=('{}')", self.arch.to_arch_name()));
        
        if let Some(ref url) = self.url {
            lines.push(format!("url=\"{}\"", url));
        }
        
        lines.push(format!("license=('{}')", self.license.to_pkgbuild()));

        // Dependencies
        let deps: Vec<String> = self.get_deps(DependencyType::Depends)
            .iter()
            .chain(self.get_deps(DependencyType::PreDepends).iter())
            .filter(|d| d.is_mapped())
            .map(|d| format!("'{}'", d.to_arch_string()))
            .collect();
        if !deps.is_empty() {
            lines.push(format!("depends=({})", deps.join(" ")));
        }

        // Optional dependencies
        let optdeps: Vec<String> = self.get_deps(DependencyType::Recommends)
            .iter()
            .chain(self.get_deps(DependencyType::Suggests).iter())
            .filter(|d| d.is_mapped())
            .map(|d| format!("'{}'", d.to_arch_string()))
            .collect();
        if !optdeps.is_empty() {
            lines.push(format!("optdepends=({})", optdeps.join(" ")));
        }

        // Conflicts
        let conflicts: Vec<String> = self.get_deps(DependencyType::Conflicts)
            .iter()
            .chain(self.get_deps(DependencyType::Breaks).iter())
            .filter(|d| d.is_mapped())
            .map(|d| format!("'{}'", d.to_arch_string()))
            .collect();
        if !conflicts.is_empty() {
            lines.push(format!("conflicts=({})", conflicts.join(" ")));
        }

        // Replaces
        let replaces: Vec<String> = self.get_deps(DependencyType::Replaces)
            .iter()
            .filter(|d| d.is_mapped())
            .map(|d| format!("'{}'", d.to_arch_string()))
            .collect();
        if !replaces.is_empty() {
            lines.push(format!("replaces=({})", replaces.join(" ")));
        }

        // Provides
        let provides: Vec<String> = self.get_deps(DependencyType::Provides)
            .iter()
            .filter(|d| d.is_mapped())
            .map(|d| format!("'{}'", d.to_arch_string()))
            .collect();
        if !provides.is_empty() {
            lines.push(format!("provides=({})", provides.join(" ")));
        }

        lines.push(String::new());
        lines.push("package() {".to_string());
        lines.push("    cp -a \"$srcdir\"/* \"$pkgdir\"/".to_string());
        lines.push("}".to_string());

        lines.join("\n")
    }
}

impl Default for PackageMetadata {
    fn default() -> Self {
        Self::new("unknown", "0.0.0")
    }
}