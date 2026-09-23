//! Package parsers for different formats

pub mod appimage;
pub mod deb;
pub mod rpm;

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::models::{PackageFormat, PackageMetadata};

/// Trait for parsing packages in various formats.
///
/// Each format (deb, rpm, AppImage, ...) implements this trait to provide
/// a uniform interface for package extraction and metadata parsing.
pub trait Parser: Send + Sync {
    /// Create a new parser for the given package file
    fn new(path: &Path) -> Result<Self>
    where
        Self: Sized;

    /// Parse the package and return metadata
    fn parse(&self) -> Result<PackageMetadata>;

    /// Get the directory where the package data was extracted
    fn extract_dir(&self) -> &Path;

    /// Get the package format this parser handles
    fn format(&self) -> PackageFormat;

    /// Persist the temporary extraction directory and return its path
    ///
    /// Used for `--keep-temp`: consumes the parser without deleting the
    /// extracted trees so they can be inspected.
    fn persist(self: Box<Self>) -> PathBuf;
}

/// Detect the package format from file contents (falling back to the
/// extension) and create the appropriate parser.
pub fn detect_and_create(path: &Path) -> Result<Box<dyn Parser + Send + Sync>> {
    // Magic bytes win over extensions (handles renamed/misnamed files)
    if rpm::is_rpm(path) {
        return Ok(Box::new(rpm::RpmParser::new(path)?));
    }
    if appimage::is_appimage(path) {
        return Ok(Box::new(appimage::AppImageParser::new(path)?));
    }
    if is_deb(path) {
        return Ok(Box::new(deb::DebParser::new(path)?));
    }

    let format = PackageFormat::from_path(path).ok_or_else(|| {
        crate::error::RexebError::UnsupportedFormat(
            format!("Cannot detect format for: {}", path.display())
        )
    })?;

    match format {
        PackageFormat::Deb => Ok(Box::new(deb::DebParser::new(path)?)),
        PackageFormat::Rpm => Ok(Box::new(rpm::RpmParser::new(path)?)),
        PackageFormat::Apk => Err(crate::error::RexebError::UnsupportedFormat(
            "APK parsing not yet implemented".into(),
        )),
        PackageFormat::AppImage => Ok(Box::new(appimage::AppImageParser::new(path)?)),
        PackageFormat::ArchPkg => Err(crate::error::RexebError::UnsupportedFormat(
            "Arch package parsing not yet implemented".into(),
        )),
    }
}

/// Quick magic-byte check for `.deb` files (`ar` archive magic)
fn is_deb(path: &Path) -> bool {
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut magic = [0u8; 8];
    match file.read_exact(&mut magic) {
        Ok(()) => magic == *b"!<arch>\n",
        Err(_) => false,
    }
}