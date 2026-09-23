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
pub mod watermark;
#[cfg(feature = "tui")]
pub mod tui;

// Re-export commonly used types
pub use error::{RexebError, Result};
pub use models::{Architecture, Dependency, PackageFormat, PackageMetadata};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Library name
pub const NAME: &str = env!("CARGO_PKG_NAME");

/// Compute a safe default for parallel jobs
///
/// Honors an explicit override, otherwise applies low-RAM / low-core safety
/// caps so dual-core and 2 GB devices are not saturated:
/// `MemAvailable < 1 GB` → 1 job; `≤2 cores` → 2 jobs; otherwise
/// `min(cores/2, 4)`.
pub fn effective_parallel_jobs(explicit: Option<usize>) -> usize {
    if let Some(jobs) = explicit {
        return jobs.max(1);
    }
    let mem_mb = read_mem_available_mb().unwrap_or(4096);
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2);
    if mem_mb < 1024 {
        tracing::warn!(
            "Low memory detected ({} MB) — capping parallel jobs",
            mem_mb
        );
        1
    } else if cores <= 2 {
        // Dual-core / i3-like: don't saturate both cores
        2.min(cores)
    } else {
        // Cap to half cores on low-end, leave headroom for the system
        (cores / 2).clamp(2, 4)
    }
}

/// Read available memory in MB from /proc/meminfo (Linux), with fallback
fn read_mem_available_mb() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/meminfo").ok()?;
    for line in content.lines() {
        if line.starts_with("MemAvailable:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                if let Ok(kb) = parts[1].parse::<u64>() {
                    return Some(kb / 1024);
                }
            }
        }
    }
    // Fallback to MemFree if MemAvailable not present (older kernels)
    for line in content.lines() {
        if line.starts_with("MemFree:") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                if let Ok(kb) = parts[1].parse::<u64>() {
                    return Some(kb / 1024);
                }
            }
        }
    }
    None
}

/// Build timestamp for generated package metadata
///
/// Honors `SOURCE_DATE_EPOCH` for reproducible builds, falling back to now.
pub fn build_timestamp() -> i64 {
    std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or_else(|| chrono::Utc::now().timestamp())
}

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
    use parsers::detect_and_create;
    use resolver::DependencyResolver;

    // Parse the package (auto-detect format)
    let parser = detect_and_create(input)?;
    let mut metadata = parser.parse()?;

    // Normalize version
    metadata.normalize_version();

    // Resolve dependencies
    let resolver = DependencyResolver::new()?;
    resolver.resolve(&mut metadata).await?;

    // Build the package
    let source_name = input
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("package");
    let converter = PackageConverter::new(metadata, parser.extract_dir())?
        .with_overwrite(true)
        .with_source_file(source_name);
    converter.build(output_dir, OutputFormat::PkgTarZst)
}

/// Analyze a package without converting
///
/// Dependencies are resolved first so the report reflects mapped versus
/// unmapped dependencies instead of listing everything as unmapped.
///
/// # Arguments
///
/// * `input` - Path to the input package file
///
/// # Returns
///
/// Analysis report on success
pub async fn analyze(input: &std::path::Path) -> Result<analyzer::AnalysisReport> {
    use parsers::detect_and_create;
    use resolver::DependencyResolver;

    let parser = detect_and_create(input)?;
    let mut metadata = parser.parse()?;
    metadata.normalize_version();

    let resolver = DependencyResolver::new()?;
    resolver.resolve(&mut metadata).await?;

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
    use parsers::detect_and_create;

    let parser = detect_and_create(input)?;
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
