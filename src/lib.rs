//! Rexeb - A smarter, faster debtap alternative
//!
//! Rexeb converts Debian (.deb) packages to Arch Linux packages with
//! intelligent dependency resolution, advanced script translation,
//! and comprehensive pre-conversion analysis.
//!
//! # Features
//!
//! - **Fast**: Written in Rust with parallel processing
//! - **Intelligent**: AI-powered fuzzy matching for dependencies
//! - **Safe**: Pre-conversion analysis and conflict detection
//! - **Flexible**: Supports multiple output formats
//! - **Extensible**: Plugin architecture for additional formats
//!
//! # Quick Start
//!
//! ```bash
//! # Convert a .deb package
//! rexeb convert package.deb
//!
//! # Convert and install
//! rexeb install package.deb
//!
//! # Analyze without converting
//! rexeb analyze package.deb
//!
//! # Update package databases
//! rexeb update --all
//! ```

#![warn(missing_docs)]
#![warn(clippy::all)]

pub mod analyzer;
pub mod cli;
pub mod config;
pub mod converter;
pub mod error;
pub mod models;
pub mod parsers;
pub mod resolver;
pub mod sandbox;
#[cfg(feature = "tui")]
pub mod tui;

// Re-export commonly used types
pub use error::{RexebError, Result};
pub use models::{Architecture, Dependency, PackageFormat, PackageMetadata};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Library name
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// Quick conversion function for simple use cases
///
/// # Arguments
///
/// * `input` - Path to the input .deb file
/// * `output_dir` - Directory to place the output package
///
/// # Returns
///
/// Path to the created package on success
///
/// # Example
///
/// ```no_run
/// use std::path::Path;
///
/// #[tokio::main]
/// async fn main() -> Result<(), Box<dyn std::error::Error>> {
///     let output = rexeb::convert(
///         Path::new("package.deb"),
///         Path::new("./output/")
///     ).await?;
///
///     println!("Created: {}", output.display());
///     Ok(())
/// }
/// ```
pub async fn convert(
    input: &std::path::Path,
    output_dir: &std::path::Path,
) -> Result<std::path::PathBuf> {
    use cli::OutputFormat;
    use converter::PackageConverter;
    use parsers::deb::DebParser;
    use resolver::DependencyResolver;

    // Parse the package
    let parser = DebParser::new(input)?;
    let mut metadata = parser.parse()?;

    // Normalize version
    metadata.normalize_version();

    // Resolve dependencies
    let resolver = DependencyResolver::new()?;
    resolver.resolve(&mut metadata).await?;

    // Build the package
    let converter = PackageConverter::new(metadata, parser.extract_dir())?;
    converter.build(output_dir, OutputFormat::PkgTarZst)
}

/// Analyze a package without converting
///
/// # Arguments
///
/// * `input` - Path to the input package file
///
/// # Returns
///
/// Analysis report on success
pub fn analyze(input: &std::path::Path) -> Result<analyzer::AnalysisReport> {
    use parsers::deb::DebParser;

    let parser = DebParser::new(input)?;
    let metadata = parser.parse()?;

    let analyzer = analyzer::PackageAnalyzer::new(&metadata, parser.extract_dir())?;
    analyzer.analyze(true, true)
}

/// Get package information
///
/// # Arguments
///
/// * `input` - Path to the input package file
///
/// # Returns
///
/// Package metadata on success
pub fn info(input: &std::path::Path) -> Result<PackageMetadata> {
    use parsers::deb::DebParser;

    let parser = DebParser::new(input)?;
    parser.parse()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        assert!(!VERSION.is_empty());
    }

    #[test]
    fn test_name() {
        assert_eq!(NAME, "rexeb");
    }
}
