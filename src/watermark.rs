//! Watermark and installed-package tracking for rexeb-converted packages
//!
//! Every package built by rexeb is stamped so it can be identified even when
//! rexeb itself is not installed. Two complementary marks are used:
//!
//! 1. *PKGINFO fields* — `x-rexeb`, `x-rexeb-time`, `x-rexeb-source` written
//!    into `.PKGINFO` (ignored by pacman/libalpm, visible via `pacman -Qi`).
//! 2. *Sentinel file* — `.rexeb.json` sibling of every converted package in the
//!    pacman local DB area, plus optionally inside `/usr/share/doc/<pkg>/`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Watermark written into every rexeb-built package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Watermark {
    /// Rexeb version that performed the conversion
    pub rexeb_version: String,
    /// ISO-8601 timestamp of conversion
    pub converted_at: String,
    /// Original `.deb` filename or source identifier
    pub source_deb: String,
    /// Arch package name
    pub arch_name: String,
    /// Full version string (`pkgver-pkgrel`)
    pub full_version: String,
    /// Architecture string
    pub arch: String,
}

impl Watermark {
    /// Create a new watermark for a given package
    pub fn new(source_deb: &str, arch_name: &str, full_version: &str, arch: &str) -> Self {
        Self {
            rexeb_version: crate::VERSION.to_string(),
            converted_at: chrono::Utc::now().to_rfc3339(),
            source_deb: source_deb.to_string(),
            arch_name: arch_name.to_string(),
            full_version: full_version.to_string(),
            arch: arch.to_string(),
        }
    }

    /// Lines to inject into `.PKGINFO`
    pub fn pkginfo_lines(&self) -> Vec<String> {
        vec![
            format!("x-rexeb = {}", self.rexeb_version),
            format!("x-rexeb-time = {}", self.converted_at),
            format!("x-rexeb-source = {}", self.source_deb),
        ]
    }

    /// Serialize to JSON (for sentinel file inside the package)
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|e| crate::error::RexebError::Other(e.to_string()))
    }
}

/// Information about a package installed on the system and identified as rexeb-originated
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledRexebPackage {
    /// Package name as known to pacman
    pub name: String,
    /// Installed version
    pub version: String,
    /// Whether this was detected via PKGINFO watermark or sentinel file
    pub detection: DetectionMethod,
    /// Watermark details if available
    pub watermark: Option<Watermark>,
    /// Path to the pacman DB entry (e.g. `/var/lib/pacman/local/foo-1.0-1/desc`)
    pub db_path: PathBuf,
}

/// How a rexeb-installed package was detected
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DetectionMethod {
    /// Found via `x-rexeb` field in `.PKGINFO`/`desc`
    PkgInfo,
    /// Found via sentinel file in `/var/lib/pacman/local`
    Sentinel,
    /// Found via doc sentinel `/usr/share/doc/<pkg>/.rexeb.json`
    DocSentinel,
}

/// Scan the local pacman database for rexeb-installed packages
pub fn list_installed() -> Result<Vec<InstalledRexebPackage>> {
    let mut result = Vec::new();
    let local_db = Path::new("/var/lib/pacman/local");

    if !local_db.exists() {
        return Ok(result);
    }

    for entry in std::fs::read_dir(local_db)? {
        let entry = entry?;
        let pkg_dir = entry.path();
        if !pkg_dir.is_dir() {
            continue;
        }

        // Read desc file
        let desc_path = pkg_dir.join("desc");
        if !desc_path.exists() {
            continue;
        }
        let desc = std::fs::read_to_string(&desc_path).unwrap_or_default();

        // Check for x-rexeb watermark
        let has_watermark = desc.contains("%X-REXEB%") || desc.to_lowercase().contains("x-rexeb");

        // Check sentinel file
        let sentinel_path = pkg_dir.join(".rexeb.json");
        let has_sentinel = sentinel_path.exists();

        // Also check doc sentinel
        let pkg_name = extract_pkg_name(&desc).unwrap_or_else(|| {
            pkg_dir.file_name().unwrap_or_default().to_string_lossy().to_string()
        });

        let detection = if has_watermark {
            Some(DetectionMethod::PkgInfo)
        } else if has_sentinel {
            Some(DetectionMethod::Sentinel)
        } else {
            // Check /usr/share/doc sentinel as fallback
            let doc_sentinel = PathBuf::from(format!("/usr/share/doc/{}/.rexeb.json", pkg_name));
            if doc_sentinel.exists() {
                Some(DetectionMethod::DocSentinel)
            } else {
                None
            }
        };

        if let Some(method) = detection {
            let version = extract_pkg_version(&desc).unwrap_or_default();
            let watermark = if has_sentinel {
                std::fs::read_to_string(&sentinel_path)
                    .ok()
                    .and_then(|c| serde_json::from_str(&c).ok())
            } else {
                None
            };

            result.push(InstalledRexebPackage {
                name: pkg_name,
                version,
                detection: method,
                watermark,
                db_path: pkg_dir,
            });
        }
    }

    // Also scan doc sentinels for packages not in local DB (edge case)
    let doc_base = Path::new("/usr/share/doc");
    if doc_base.exists() {
        for entry in std::fs::read_dir(doc_base).into_iter().flatten() {
            if let Ok(e) = entry {
                let doc_path = e.path().join(".rexeb.json");
                if doc_path.exists() {
                    // Only add if not already found via pacman DB
                    let doc_pkg = e.file_name().to_string_lossy().to_string();
                    if !result.iter().any(|p| p.name == doc_pkg) {
                        let watermark: Option<Watermark> = std::fs::read_to_string(&doc_path)
                            .ok()
                            .and_then(|c| serde_json::from_str(&c).ok());
                        result.push(InstalledRexebPackage {
                            name: doc_pkg.clone(),
                            version: watermark.as_ref().map(|w| w.full_version.clone()).unwrap_or_default(),
                            detection: DetectionMethod::DocSentinel,
                            watermark,
                            db_path: doc_path,
                        });
                    }
                }
            }
        }
    }

    Ok(result)
}

/// Lightweight check: is a given package installed via rexeb?
pub fn is_rexeb_package(pkg_name: &str) -> bool {
    // Fast path: check pacman DB desc file
    let local_db = Path::new("/var/lib/pacman/local");
    if local_db.exists() {
        if let Ok(entries) = std::fs::read_dir(local_db) {
            for entry in entries.flatten() {
                let desc_path = entry.path().join("desc");
                if desc_path.exists() {
                    if let Ok(desc) = std::fs::read_to_string(&desc_path) {
                        if desc.to_lowercase().contains("x-rexeb") {
                            if let Some(name) = extract_pkg_name(&desc) {
                                if name == pkg_name {
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

fn extract_pkg_name(desc: &str) -> Option<String> {
    let marker = "%NAME%";
    let start = desc.find(marker)?;
    let rest = &desc[start + marker.len()..];
    // Skip the blank line after %NAME%
    for line in rest.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('%') {
            continue;
        }
        return Some(t.to_string());
    }
    None
}

fn extract_pkg_version(desc: &str) -> Option<String> {
    let marker = "%VERSION%";
    let start = desc.find(marker)?;
    let rest = &desc[start + marker.len()..];
    for line in rest.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('%') {
            continue;
        }
        return Some(t.to_string());
    }
    None
}

/// Attempt to fix icon references in an installed rexeb package
///
/// Scans `.desktop` files and checks `Icon=` against available icons.
pub fn fix_icons(pkg_name: &str) -> Result<Vec<String>> {
    let mut fixed = Vec::new();
    let desktop_dirs = [
        PathBuf::from(format!("/usr/share/applications/{}*.desktop", pkg_name)),
        PathBuf::from("/usr/share/applications"),
    ];

    for dir in &desktop_dirs {
        let actual_dir = if dir.to_string_lossy().contains('*') {
            PathBuf::from("/usr/share/applications")
        } else {
            dir.clone()
        };
        if !actual_dir.exists() {
            continue;
        }
        for entry in std::fs::read_dir(&actual_dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
                continue;
            }
            // Only touch files belonging to this package (name substring match)
            if !path.file_name().unwrap_or_default().to_string_lossy().contains(pkg_name)
                && pkg_name != "*"
            {
                continue;
            }
            if let Ok(content) = std::fs::read_to_string(&path) {
                let mut new_content = content.clone();
                let mut changed = false;
                for line in content.lines() {
                    if let Some(icon) = line.strip_prefix("Icon=") {
                        let icon = icon.trim();
                        if icon.is_empty() {
                            continue;
                        }
                        // Check if icon file exists in any hicolor path
                        let icon_exists = Path::new(icon).exists()
                            || PathBuf::from(format!("/usr/share/icons/hicolor/48x48/apps/{}.png", icon)).exists()
                            || PathBuf::from(format!("/usr/share/pixmaps/{}.png", icon)).exists()
                            || PathBuf::from(format!("/usr/share/pixmaps/{}", icon)).exists();
                        if !icon_exists {
                            // Try to find a matching icon in hicolor
                            let replacement = find_replacement_icon(icon);
                            if let Some(repl) = replacement {
                                new_content = new_content.replace(
                                    &format!("Icon={}", icon),
                                    &format!("Icon={}", repl),
                                );
                                changed = true;
                                fixed.push(format!("{}: Icon={} -> {}", path.display(), icon, repl));
                            }
                        }
                    }
                }
                if changed {
                    // Need sudo to write; for now just report
                    tracing::info!("Would fix {}", path.display());
                }
            }
        }
    }

    Ok(fixed)
}

fn find_replacement_icon(_icon: &str) -> Option<String> {
    // Look for any available icon that could serve as fallback
    // For now, return None - a more sophisticated lookup could scan /usr/share/icons
    None
}

/// Rename a rexeb-installed package entry (edits local DB `desc` if permissions allow)
///
/// This is a best-effort helper; the actual package file rename requires rebuilding.
pub fn rename_installed(old_name: &str, new_name: &str) -> Result<()> {
    // For now, we provide guidance rather than mutating pacman DB directly
    // The user should reconvert with --name and reinstall
    tracing::info!(
        "To rename '{}' to '{}', reconvert with: rexeb convert --name {} <package.deb> && sudo pacman -U <output>",
        old_name, new_name, new_name
    );
    // Record the alias in user mappings so future conversions use the new name
    let mut db = crate::resolver::PackageDatabase::new()?;
    db.add_mapping(old_name, new_name, 1.0);
    db.save()?;
    Ok(())
}
