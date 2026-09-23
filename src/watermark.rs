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
    /// Original source filename (`.deb`/`.rpm`/`.AppImage`) or identifier
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
/// Scans `.desktop` files, checks each `Icon=` against the installed icon
/// themes, and rewrites broken references to the closest available icon
/// (falling back to a generic icon). Originals are backed up to
/// `<file>.desktop.rexeb-bak` before writing. Requires write access to
/// `/usr/share/applications` (i.e. run as root).
pub fn fix_icons(pkg_name: &str) -> Result<Vec<String>> {
    let mut fixed = Vec::new();
    let apps_dir = Path::new("/usr/share/applications");
    if !apps_dir.exists() {
        return Ok(fixed);
    }

    for entry in std::fs::read_dir(apps_dir)?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("desktop") {
            continue;
        }
        // Only touch files belonging to this package (name substring match)
        if pkg_name != "*"
            && !path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .contains(pkg_name)
        {
            continue;
        }
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let mut new_content = content.clone();
        let mut changed = false;
        for line in content.lines() {
            if let Some(icon) = line.strip_prefix("Icon=") {
                let icon = icon.trim();
                if icon.is_empty() || icon_exists(icon) {
                    continue;
                }
                let replacement = find_replacement_icon(icon)
                    .unwrap_or_else(|| "application-x-executable".to_string());
                new_content = new_content.replace(
                    &format!("Icon={}", icon),
                    &format!("Icon={}", replacement),
                );
                changed = true;
                fixed.push(format!(
                    "{}: Icon={} -> {}",
                    path.display(),
                    icon,
                    replacement
                ));
            }
        }
        if changed {
            // Back up the original, then write the fix
            let backup = path.with_extension("desktop.rexeb-bak");
            if std::fs::copy(&path, &backup).is_err() {
                fixed.push(format!(
                    "{}: could not write fix (permission denied — run as root?)",
                    path.display()
                ));
                continue;
            }
            if let Err(e) = std::fs::write(&path, new_content) {
                fixed.push(format!("{}: write failed: {}", path.display(), e));
            }
        }
    }

    Ok(fixed)
}

/// Check whether an icon name (or absolute path) resolves to a real file
fn icon_exists(icon: &str) -> bool {
    if icon.contains('/') {
        return Path::new(icon).exists();
    }
    for dir in [
        "/usr/share/icons/hicolor",
        "/usr/share/icons/Adwaita",
        "/usr/share/pixmaps",
    ] {
        for ext in ["png", "svg", "xpm"] {
            if PathBuf::from(format!("{}/{}.{}", dir, icon, ext)).exists() {
                return true;
            }
        }
    }
    // Sized hicolor subdirectories
    for size in [
        "16x16", "22x22", "24x24", "32x32", "48x48", "64x64", "128x128", "256x256", "scalable",
    ] {
        for sub in ["apps", "devices", "mimetypes"] {
            for ext in ["png", "svg", "xpm"] {
                if PathBuf::from(format!(
                    "/usr/share/icons/hicolor/{}/{}/{}.{}",
                    size, sub, icon, ext
                ))
                .exists()
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Find the closest available icon by name-prefix match
fn find_replacement_icon(icon: &str) -> Option<String> {
    let prefix = icon.split(['-', '_']).next().unwrap_or(icon);
    if prefix.len() < 3 {
        return None;
    }
    for dir in [
        "/usr/share/pixmaps",
        "/usr/share/icons/hicolor/48x48/apps",
        "/usr/share/icons/hicolor/scalable/apps",
    ] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let fname = entry.file_name().to_string_lossy().to_string();
                let stem = fname.split('.').next().unwrap_or("").to_string();
                if stem != icon && stem.starts_with(prefix) {
                    return Some(stem);
                }
            }
        }
    }
    None
}

/// Record a rename alias for a rexeb-installed package
///
/// Mutating pacman's local DB directly would corrupt it, so this only records
/// an alias for future conversions; actually renaming requires reconverting
/// with `--name` and reinstalling.
pub fn rename_installed(old_name: &str, new_name: &str) -> Result<()> {
    // For now, we provide guidance rather than mutating pacman DB directly
    // The user should reconvert with --name and reinstall
    tracing::info!(
        "To rename '{}' to '{}', reconvert with: rexeb convert --name {} <package> && sudo pacman -U <output>",
        old_name, new_name, new_name
    );
    // Record the alias in user mappings so future conversions use the new name
    let mut db = crate::resolver::PackageDatabase::new()?;
    db.add_mapping(old_name, new_name, 1.0);
    db.save()?;
    Ok(())
}
