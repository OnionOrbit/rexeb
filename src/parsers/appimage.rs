//! AppImage parser / integrator
//!
//! AppImages are self-contained ELF executables with an embedded squashfs
//! payload (magic `AI\x01`/`AI\x02` at offset 8). There is nothing to
//! "convert" dependency-wise — everything is bundled — so rexeb instead
//! *integrates* them into a proper Arch package:
//!
//! - the AppImage is extracted (`--appimage-extract`, no FUSE needed) and
//!   the AppDir tree is installed under `/opt/<name>/`
//! - the `.desktop` entry is rewritten (`Exec=/opt/<name>/AppRun`) and
//!   installed to `/usr/share/applications/`
//! - the icon goes to `/usr/share/pixmaps/`, metainfo to
//!   `/usr/share/metainfo/`
//!
//! The result has zero dependencies and uninstalls cleanly via pacman.
//! Name/version come from the `.desktop` file when available, falling back
//! to filename heuristics (`Foo-1.2.3-x86_64.AppImage`).

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

use crate::error::{RexebError, Result};
use crate::models::{Architecture, PackageFormat, PackageMetadata};
use crate::parsers::Parser;

/// Quick magic-byte check for AppImages (ELF + `AI\x01`/`AI\x02` at offset 8)
pub fn is_appimage(path: &Path) -> bool {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut header = [0u8; 12];
    match file.read_exact(&mut header) {
        Ok(()) => {
            header[0..4] == [0x7f, b'E', b'L', b'F']
                && header[8] == b'A'
                && header[9] == b'I'
                && (header[10] == 1 || header[10] == 2)
        }
        Err(_) => false,
    }
}

/// Detect the target architecture from the ELF `e_machine` field
pub fn arch_from_elf(path: &Path) -> Option<Architecture> {
    let mut file = File::open(path).ok()?;
    let mut header = [0u8; 20];
    file.read_exact(&mut header).ok()?;
    if header[0..4] != [0x7f, b'E', b'L', b'F'] {
        return None;
    }
    // EI_DATA: 1 = little-endian, 2 = big-endian
    let machine = if header[5] == 2 {
        u16::from_be_bytes([header[18], header[19]])
    } else {
        u16::from_le_bytes([header[18], header[19]])
    };
    match machine {
        3 => Some(Architecture::I686),    // EM_386
        40 => Some(Architecture::Armv7h), // EM_ARM
        62 => Some(Architecture::X86_64), // EM_X86_64
        183 => Some(Architecture::Aarch64), // EM_AARCH64
        _ => None,
    }
}

/// Parsed `.desktop` entry (subset rexeb cares about)
#[derive(Debug, Clone, Default)]
pub struct DesktopInfo {
    /// Application name
    pub name: Option<String>,
    /// Exec line
    pub exec: Option<String>,
    /// Icon reference
    pub icon: Option<String>,
    /// Comment / description
    pub comment: Option<String>,
    /// Categories
    pub categories: Option<String>,
    /// AppImage version stamp
    pub x_appimage_version: Option<String>,
    /// Raw lines of the original file (rewritten at stage time)
    lines: Vec<String>,
}

/// Metadata derived from an extracted AppDir
#[derive(Debug, Clone)]
pub struct AppImageMeta {
    /// Sanitized Arch package name
    pub name: String,
    /// Application version
    pub version: String,
    /// Description
    pub description: String,
    /// Target architecture
    pub arch: Architecture,
    /// Parsed desktop entry
    pub desktop: DesktopInfo,
    /// Resolved icon file inside the AppDir, if any
    pub icon_source: Option<PathBuf>,
}

/// Parser for AppImage files
pub struct AppImageParser {
    /// Path to the .AppImage file
    path: PathBuf,
    /// Temporary directory for extraction
    temp_dir: TempDir,
    /// Path to the staged data directory (`opt/<name>/`, `usr/...`)
    data_dir: PathBuf,
    /// Derived metadata
    meta: AppImageMeta,
    /// Installed file list (absolute paths)
    files: Vec<PathBuf>,
    /// Accumulated installed size in bytes
    installed_size: u64,
}

impl AppImageParser {
    /// Create a new parser for the given AppImage (extracts + stages it)
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        if !path.exists() {
            return Err(RexebError::file_not_found(&path));
        }
        if !is_appimage(&path) {
            return Err(RexebError::AppImageParsing(format!(
                "Not an AppImage: {}",
                path.display()
            )));
        }

        // Prefixed so `rexeb clean --temp` can find leftovers (e.g. from --keep-temp)
        let temp_dir = tempfile::Builder::new().prefix("rexeb-").tempdir()?;
        let data_dir = temp_dir.path().join("data");
        std::fs::create_dir_all(&data_dir)?;

        let approot = extract_appimage(&path, temp_dir.path())?;
        let arch = arch_from_elf(&path).unwrap_or_else(Architecture::current);
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("appimage-app");
        let meta = extract_metadata(&approot, stem, arch)?;
        let (files, installed_size) = stage_appdir(&approot, &data_dir, &meta)?;

        Ok(Self {
            path,
            temp_dir,
            data_dir,
            meta,
            files,
            installed_size,
        })
    }

    /// Path to the source AppImage file
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    /// Get the staged data directory path
    pub fn extract_dir(&self) -> &Path {
        self.data_dir.as_path()
    }

    /// Build package metadata (AppImages carry no dependencies)
    pub fn parse(&self) -> Result<PackageMetadata> {
        let mut meta = PackageMetadata::new(&self.meta.name, &self.meta.version);
        meta.arch = self.meta.arch;
        meta.description = self.meta.description.clone();
        meta.files = self.files.clone();
        meta.installed_size = self.installed_size;
        Ok(meta)
    }
}

impl Parser for AppImageParser {
    fn new(path: &Path) -> Result<Self> {
        AppImageParser::new(path)
    }

    fn parse(&self) -> Result<PackageMetadata> {
        self.parse()
    }

    fn extract_dir(&self) -> &Path {
        self.extract_dir()
    }

    fn format(&self) -> PackageFormat {
        PackageFormat::AppImage
    }

    fn persist(self: Box<Self>) -> PathBuf {
        let path = self.temp_dir.path().to_path_buf();
        std::mem::forget(self.temp_dir);
        path
    }
}

/// Run `--appimage-extract` into `work/` and return the `squashfs-root` path
///
/// Extraction needs no FUSE (the runtime unpacks squashfs itself). When the
/// file lacks the executable bit it is copied to temp first so the original
/// is never modified.
fn extract_appimage(path: &Path, work: &Path) -> Result<PathBuf> {
    let runnable: PathBuf = if is_executable(path) {
        path.to_path_buf()
    } else {
        let copy = work.join("appimage-run");
        std::fs::copy(path, &copy)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&copy, std::fs::Permissions::from_mode(0o755))?;
        }
        copy
    };

    tracing::info!("Extracting AppImage payload (this can take a while)...");
    let status = Command::new(&runnable)
        .arg("--appimage-extract")
        .current_dir(work)
        .env("APPIMAGE_EXTRACT_AND_RUN", "1")
        .status()
        .map_err(|e| RexebError::AppImageParsing(format!("Cannot execute AppImage runtime: {}", e)))?;
    if !status.success() {
        return Err(RexebError::AppImageParsing(
            "`--appimage-extract` failed (file may be truncated or corrupt)".into(),
        ));
    }
    let approot = work.join("squashfs-root");
    if !approot.join("AppRun").exists() {
        return Err(RexebError::AppImageParsing(
            "Extraction produced no squashfs-root/AppRun".into(),
        ));
    }
    Ok(approot)
}

/// Derive package metadata from the extracted AppDir + filename
pub fn extract_metadata(approot: &Path, filename_stem: &str, arch: Architecture) -> Result<AppImageMeta> {
    let desktop_path = find_desktop_file(approot).ok_or_else(|| {
        RexebError::AppImageParsing("No .desktop file found in the AppImage".into())
    })?;
    let desktop = parse_desktop_file(&desktop_path)?;
    let (stem_name, stem_version) = split_appimage_filename(filename_stem);

    let name = desktop
        .name
        .as_deref()
        .map(sanitize_pkgname)
        .filter(|n| !n.is_empty())
        .unwrap_or(stem_name);
    let name = if name.is_empty() {
        "appimage-app".to_string()
    } else {
        name
    };
    let version = desktop
        .x_appimage_version
        .as_deref()
        .map(sanitize_version)
        .filter(|v| !v.is_empty())
        .or(stem_version)
        .unwrap_or_else(|| "1.0".to_string());
    let description = desktop.comment.clone().unwrap_or_else(|| {
        format!("{} (AppImage repackaged by rexeb)", name)
    });
    let icon_source = resolve_icon(approot, desktop.icon.as_deref());

    Ok(AppImageMeta {
        name,
        version,
        description,
        arch,
        desktop,
        icon_source,
    })
}

/// Stage the AppDir into a package data tree, returning files + total size
///
/// Layout: `opt/<name>/` (full AppDir), `usr/share/applications/<name>.desktop`
/// (rewritten), `usr/share/pixmaps/<name>.<ext>` (icon), metainfo passthrough.
pub fn stage_appdir(
    approot: &Path,
    data_dir: &Path,
    meta: &AppImageMeta,
) -> Result<(Vec<PathBuf>, u64)> {
    let mut installed_size = 0u64;

    // Full AppDir under /opt/<name> (modes preserved: AppRun must stay +x)
    let opt_dir = data_dir.join("opt").join(&meta.name);
    copy_tree_preserve(approot, &opt_dir, &mut installed_size)?;

    // Icon (pixmaps dir is always on the icon path)
    let mut icon_name = meta.desktop.icon.clone().unwrap_or_else(|| meta.name.clone());
    if let Some(ref source) = meta.icon_source {
        let ext = source
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("png");
        let dest = data_dir
            .join("usr/share/pixmaps")
            .join(format!("{}.{}", meta.name, ext));
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let resolved = resolve_link(source);
        std::fs::copy(&resolved, &dest)?;
        installed_size += resolved.metadata().map(|m| m.len()).unwrap_or(0);
        icon_name = meta.name.clone();
    }

    // Desktop entry (rewritten Exec/Icon, normalized Type/Categories)
    let desktop_content = rewrite_desktop(&meta.desktop, &meta.name, &icon_name);
    let desktop_dest = data_dir
        .join("usr/share/applications")
        .join(format!("{}.desktop", meta.name));
    if let Some(parent) = desktop_dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&desktop_dest, desktop_content)?;
    installed_size += desktop_dest.metadata().map(|m| m.len()).unwrap_or(0);

    // AppStream metainfo passthrough (so GNOME Software / Discover see it)
    let metainfo_src = approot.join("usr/share/metainfo");
    if metainfo_src.is_dir() {
        let metainfo_dest = data_dir.join("usr/share/metainfo");
        std::fs::create_dir_all(&metainfo_dest)?;
        if let Ok(entries) = std::fs::read_dir(&metainfo_src) {
            for entry in entries.flatten() {
                let src = entry.path();
                if src.is_file() {
                    if let Some(file_name) = src.file_name() {
                        let dest = metainfo_dest.join(file_name);
                        if std::fs::copy(&src, &dest).is_ok() {
                            installed_size += dest.metadata().map(|m| m.len()).unwrap_or(0);
                        }
                    }
                }
            }
        }
    }

    // Installed file list (files + symlinks, absolute paths)
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(data_dir) {
        let entry = entry?;
        let file_type = entry.file_type();
        if file_type.is_file() || file_type.is_symlink() {
            if let Ok(rel) = entry.path().strip_prefix(data_dir) {
                files.push(PathBuf::from("/").join(rel));
            }
        }
    }

    Ok((files, installed_size))
}

/// Find the AppDir's `.desktop` file (top level, deterministic order)
fn find_desktop_file(approot: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(approot)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("desktop"))
        .collect();
    candidates.sort();
    candidates.into_iter().next()
}

/// Parse the `[Desktop Entry]` section of a `.desktop` file (first wins)
pub fn parse_desktop_file(path: &Path) -> Result<DesktopInfo> {
    let bytes = std::fs::read(path)?;
    let content = String::from_utf8_lossy(&bytes);
    let mut info = DesktopInfo::default();
    let mut in_entry = false;

    for line in content.lines() {
        info.lines.push(line.to_string());
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with('[') {
            in_entry = trimmed == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        let value = value.trim().to_string();
        match key.trim() {
            "Name" if info.name.is_none() => info.name = Some(value),
            "Exec" if info.exec.is_none() => info.exec = Some(value),
            "Icon" if info.icon.is_none() => info.icon = Some(value),
            "Comment" if info.comment.is_none() => info.comment = Some(value),
            "Categories" if info.categories.is_none() => info.categories = Some(value),
            "X-AppImage-Version" if info.x_appimage_version.is_none() => {
                info.x_appimage_version = Some(value)
            }
            _ => {}
        }
    }
    Ok(info)
}

/// Rewrite a desktop entry for the staged install location
///
/// `Exec` points at `/opt/<name>/AppRun` (preserving one launch field code
/// such as `%F`), `Icon` at the staged icon, `Type=Application` is ensured,
/// and `Categories` gets its mandatory trailing `;`. Everything else is
/// preserved byte-for-byte.
fn rewrite_desktop(desktop: &DesktopInfo, name: &str, icon_name: &str) -> String {
    let field_code = desktop
        .exec
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .find(|t| {
            t.len() == 2
                && t.starts_with('%')
                && matches!(
                    t.chars().nth(1),
                    Some('f' | 'F' | 'u' | 'U' | 'd' | 'D' | 'n' | 'N' | 'i' | 'c' | 'k' | 'v' | 'm')
                )
        })
        .unwrap_or("");
    let new_exec = if field_code.is_empty() {
        format!("/opt/{}/AppRun", name)
    } else {
        format!("/opt/{}/AppRun {}", name, field_code)
    };

    let mut out = Vec::new();
    let mut in_entry = false;
    let mut saw_exec = false;
    let mut saw_icon = false;
    let mut saw_type = false;

    for line in &desktop.lines {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_entry = trimmed == "[Desktop Entry]";
            out.push(line.clone());
            continue;
        }
        if in_entry {
            if let Some((key, _)) = trimmed.split_once('=') {
                match key.trim() {
                    "Exec" => {
                        out.push(format!("Exec={}", new_exec));
                        saw_exec = true;
                        continue;
                    }
                    "Icon" => {
                        out.push(format!("Icon={}", icon_name));
                        saw_icon = true;
                        continue;
                    }
                    "Type" => {
                        saw_type = true;
                    }
                    "Categories" => {
                        let mut value = trimmed.split_once('=').map(|(_, v)| v.trim().to_string()).unwrap_or_default();
                        if !value.ends_with(';') {
                            value.push(';');
                        }
                        out.push(format!("Categories={}", value));
                        continue;
                    }
                    _ => {}
                }
            }
        }
        out.push(line.clone());
    }

    if !saw_exec {
        out.push(format!("Exec={}", new_exec));
    }
    if !saw_icon {
        out.push(format!("Icon={}", icon_name));
    }
    if !saw_type {
        out.push("Type=Application".to_string());
    }

    let mut content = out.join("\n");
    content.push('\n');
    content
}

/// Resolve an icon reference to a file inside the AppDir
///
/// Handles absolute-in-AppDir paths, theme-style names (searched in the
/// usual locations), and the `.DirIcon` fallback.
fn resolve_icon(approot: &Path, icon: Option<&str>) -> Option<PathBuf> {
    if let Some(icon) = icon {
        let icon = icon.trim();
        if !icon.is_empty() {
            if icon.contains('/') {
                let candidate = approot.join(icon.trim_start_matches('/'));
                if candidate.is_file() || candidate.is_symlink() {
                    return Some(candidate);
                }
            } else {
                for dir in [
                    approot.to_path_buf(),
                    approot.join("usr/share/pixmaps"),
                    approot.join("usr/share/icons"),
                    approot.join(".icons"),
                ] {
                    for ext in ["png", "svg", "xpm", "ico"] {
                        let candidate = dir.join(format!("{}.{}", icon, ext));
                        if candidate.is_file() || candidate.is_symlink() {
                            return Some(candidate);
                        }
                    }
                    // hicolor tree: usr/share/icons/hicolor/*/apps/<icon>.*
                    if dir.ends_with("icons") {
                        if let Ok(hicolor) = std::fs::read_dir(dir.join("hicolor")) {
                            for size in hicolor.flatten() {
                                for ext in ["png", "svg", "xpm"] {
                                    let candidate = size
                                        .path()
                                        .join("apps")
                                        .join(format!("{}.{}", icon, ext));
                                    if candidate.is_file() || candidate.is_symlink() {
                                        return Some(candidate);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // `.DirIcon` fallback (often a symlink to the real icon)
    let dir_icon = approot.join(".DirIcon");
    if dir_icon.is_file() || dir_icon.is_symlink() {
        return Some(dir_icon);
    }
    None
}

/// Follow a symlink chain (one level is the common case)
fn resolve_link(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    for _ in 0..8 {
        if !current.is_symlink() {
            break;
        }
        let target = match std::fs::read_link(&current) {
            Ok(t) => t,
            Err(_) => break,
        };
        current = if target.is_absolute() {
            target
        } else {
            current
                .parent()
                .unwrap_or_else(|| Path::new("/"))
                .join(target)
        };
    }
    current
}

/// Split `Foo-Bar-1.2.3-x86_64` into (sanitized name, version?)
///
/// The version is the first `-`/`_`-separated component starting with a
/// digit; the name is everything before it. Without a version component a
/// trailing architecture token is dropped instead.
pub fn split_appimage_filename(stem: &str) -> (String, Option<String>) {
    let parts: Vec<&str> = stem
        .split(['-', '_'])
        .filter(|p| !p.is_empty())
        .collect();
    if parts.is_empty() {
        return ("appimage-app".to_string(), None);
    }

    let mut version_idx = None;
    for (i, part) in parts.iter().enumerate() {
        if part.chars().next().map_or(false, |c| c.is_ascii_digit()) {
            version_idx = Some(i);
            break;
        }
    }

    match version_idx {
        Some(0) => (
            "appimage-app".to_string(),
            Some(sanitize_version(parts[0])),
        ),
        Some(i) => (
            sanitize_pkgname(&parts[..i].join("-")),
            Some(sanitize_version(parts[i])),
        ),
        None => {
            let mut end = parts.len();
            if end > 1 && is_arch_token(parts[end - 1]) {
                end -= 1;
            }
            (sanitize_pkgname(&parts[..end].join("-")), None)
        }
    }
}

/// Whether a filename component is an architecture token
fn is_arch_token(part: &str) -> bool {
    matches!(
        part.to_lowercase().as_str(),
        "x86_64" | "x64" | "amd64" | "aarch64" | "arm64" | "i386" | "i686" | "armhf"
            | "armv7l" | "riscv64" | "ppc64le" | "s390x" | "linux"
    )
}

/// Sanitize a display name into a valid Arch package name
pub fn sanitize_pkgname(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut last_dash = true; // trim leading dashes
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || "@._+".contains(c) {
            out.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    let trimmed = out.trim_matches(|c| c == '-' || c == '.').to_string();
    if trimmed.is_empty() {
        "appimage-app".to_string()
    } else {
        trimmed
    }
}

/// Sanitize a version string (`-`/`:` would confuse pkgver/pkgrel/epoch splits)
pub fn sanitize_version(version: &str) -> String {
    let mut out: String = version
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "._+~^".contains(c) {
                c
            } else {
                '.'
            }
        })
        .collect();
    out = out.trim_matches('.').to_string();
    if out.is_empty() {
        "1.0".to_string()
    } else {
        out
    }
}

/// Recursively copy a tree, preserving modes and symlinks
fn copy_tree_preserve(src_root: &Path, dst_root: &Path, installed_size: &mut u64) -> Result<()> {
    for entry in walkdir::WalkDir::new(src_root) {
        let entry = entry?;
        let source = entry.path();
        let rel_path = match source.strip_prefix(src_root) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if rel_path.as_os_str().is_empty() {
            continue;
        }
        let dest = dst_root.join(rel_path);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&dest)?;
            copy_mode(source, &dest);
        } else if entry.file_type().is_symlink() {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            #[cfg(unix)]
            {
                let target = std::fs::read_link(source)?;
                if dest.exists() || dest.symlink_metadata().is_ok() {
                    let _ = std::fs::remove_file(&dest);
                }
                std::os::unix::fs::symlink(target, &dest)?;
            }
        } else if entry.file_type().is_file() {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(source, &dest)?;
            copy_mode(source, &dest);
            *installed_size += source.metadata().map(|m| m.len()).unwrap_or(0);
        }
    }
    Ok(())
}

/// Best-effort permission-bit copy (keeps AppRun +x)
#[cfg(unix)]
fn copy_mode(source: &Path, dest: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(mode) = source.metadata().map(|m| m.permissions().mode()) {
        let _ = std::fs::set_permissions(dest, std::fs::Permissions::from_mode(mode & 0o7777));
    }
}

/// Non-Unix builds have no mode bits to preserve
#[cfg(not(unix))]
fn copy_mode(_source: &Path, _dest: &Path) {}

/// Whether a file has any executable bit set
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Non-Unix builds assume executability
#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}
