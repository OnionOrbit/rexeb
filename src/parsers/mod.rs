//! Package parsers for different formats

pub mod deb;

use std::path::Path;

use crate::error::Result;
use crate::models::{PackageFormat, PackageMetadata};

/// Trait for parsing packages in various formats.
///
/// Each format (deb, rpm, apk, AppImage) should implement this trait
/// to provide a uniform interface for package extraction and metadata parsing.
pub trait Parser {
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
}

/// Detect the package format from a file path and create the appropriate parser.
///
/// Returns `None` if the format is unsupported.
pub fn detect_and_create(path: &Path) -> Result<Box<dyn Parser>> {
    let format = PackageFormat::from_path(path).ok_or_else(|| {
        crate::error::RexebError::UnsupportedFormat(
            format!("Cannot detect format for: {}", path.display())
        )
    })?;

    match format {
        PackageFormat::Deb => Ok(Box::new(deb::DebParser::new(path)?)),
        PackageFormat::Rpm => Err(crate::error::RexebError::UnsupportedFormat(
            "RPM parsing not yet implemented".into()
        )),
        PackageFormat::Apk => Err(crate::error::RexebError::UnsupportedFormat(
            "APK parsing not yet implemented".into()
        )),
        PackageFormat::AppImage => Err(crate::error::RexebError::UnsupportedFormat(
            "AppImage parsing not yet implemented".into()
        )),
        PackageFormat::ArchPkg => Err(crate::error::RexebError::UnsupportedFormat(
            "Arch package parsing not yet implemented".into()
        )),
    }
}