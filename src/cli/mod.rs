//! Command-line interface for rexeb

mod commands;

pub use commands::*;

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Rexeb - A smarter, faster debtap alternative
/// 
/// Convert .deb packages to Arch Linux packages with intelligent
/// dependency resolution and advanced features.
#[derive(Parser, Debug)]
#[command(name = "rexeb")]
#[command(author, version, about, long_about = None)]
#[command(propagate_version = true)]
pub struct Cli {
    /// Subcommand to execute
    #[command(subcommand)]
    pub command: Commands,

    /// Enable verbose output
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Suppress non-essential output
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Configuration file path
    #[arg(short, long, global = true, env = "REXEB_CONFIG")]
    pub config: Option<PathBuf>,

    /// Number of parallel jobs (default: number of CPUs)
    #[arg(short, long, global = true)]
    pub jobs: Option<usize>,

    /// Use TUI interface
    #[arg(long, global = true)]
    pub tui: bool,
}

/// Available commands
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Convert a package to Arch Linux format
    Convert(ConvertArgs),

    /// Update package databases and mappings
    Update(UpdateArgs),

    /// Show information about a package
    Info(InfoArgs),

    /// Search for package mappings
    Search(SearchArgs),

    /// Analyze a package without converting
    Analyze(AnalyzeArgs),

    /// Install a package (convert and install in one step)
    Install(InstallArgs),

    /// Manage configuration
    Config(ConfigArgs),

    /// Clean cache and temporary files
    Clean(CleanArgs),
}

/// Arguments for the convert command
#[derive(Parser, Debug, Clone)]
pub struct ConvertArgs {
    /// Input package file(s)
    #[arg(required = true)]
    pub input: Vec<PathBuf>,

    /// Output directory (default: current directory)
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Skip dependency resolution
    #[arg(long)]
    pub skip_deps: bool,

    /// Force conversion even with warnings
    #[arg(short, long)]
    pub force: bool,

    /// Generate PKGBUILD instead of binary package
    #[arg(short, long)]
    pub pkgbuild: bool,

    /// Skip interactive prompts (use defaults)
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Treat package as 64-bit (pseudo-64-bit mode)
    #[arg(short = 'P', long)]
    pub pseudo64: bool,

    /// Keep temporary files after conversion
    #[arg(long)]
    pub keep_temp: bool,

    /// Custom package name override
    #[arg(long)]
    pub name: Option<String>,

    /// Custom version override
    #[arg(long)]
    pub version_override: Option<String>,

    /// Custom package release number
    #[arg(long)]
    pub release: Option<String>,

    /// Output format
    #[arg(long, value_enum, default_value_t = OutputFormat::PkgTarZst)]
    pub format: OutputFormat,
}

/// Output format for converted packages
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// .pkg.tar.zst (default, recommended)
    PkgTarZst,
    /// .pkg.tar.xz (legacy format)
    PkgTarXz,
    /// .pkg.tar.gz (for compatibility)
    PkgTarGz,
}

impl OutputFormat {
    /// Get file extension
    pub fn extension(&self) -> &'static str {
        match self {
            Self::PkgTarZst => "pkg.tar.zst",
            Self::PkgTarXz => "pkg.tar.xz",
            Self::PkgTarGz => "pkg.tar.gz",
        }
    }
}

/// Arguments for the update command
#[derive(Parser, Debug)]
pub struct UpdateArgs {
    /// Update virtual packages database
    #[arg(long)]
    pub virtual_packages: bool,

    /// Update package name mappings
    #[arg(short, long)]
    pub mappings: bool,

    /// Update AUR package cache
    #[arg(short, long)]
    pub aur: bool,

    /// Update all databases
    #[arg(short = 'A', long)]
    pub all: bool,

    /// Force update even if recently updated
    #[arg(short, long)]
    pub force: bool,
}

/// Arguments for the info command
#[derive(Parser, Debug)]
pub struct InfoArgs {
    /// Package file or name to get info about
    #[arg(required = true)]
    pub package: PathBuf,

    /// Output format
    #[arg(short, long, value_enum, default_value_t = InfoFormat::Pretty)]
    pub format: InfoFormat,

    /// Show extended information
    #[arg(short, long)]
    pub extended: bool,
}

/// Info output format
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InfoFormat {
    /// Human-readable pretty output
    Pretty,
    /// JSON output
    Json,
    /// TOML output
    Toml,
}

/// Arguments for the search command
#[derive(Parser, Debug)]
pub struct SearchArgs {
    /// Package name to search for
    #[arg(required = true)]
    pub query: String,

    /// Search in Arch repositories
    #[arg(short, long)]
    pub arch: bool,

    /// Search in AUR
    #[arg(short = 'A', long)]
    pub aur: bool,

    /// Maximum results to show
    #[arg(short, long, default_value = "20")]
    pub limit: usize,

    /// Include fuzzy matches
    #[arg(short, long)]
    pub fuzzy: bool,
}

/// Arguments for the analyze command
#[derive(Parser, Debug)]
pub struct AnalyzeArgs {
    /// Package file to analyze
    #[arg(required = true)]
    pub input: PathBuf,

    /// Check for conflicts with installed packages
    #[arg(long)]
    pub conflicts: bool,

    /// Verify file integrity
    #[arg(long)]
    pub verify: bool,

    /// Output format
    #[arg(short, long, value_enum, default_value_t = InfoFormat::Pretty)]
    pub format: InfoFormat,
}

/// Arguments for the install command
#[derive(Parser, Debug)]
pub struct InstallArgs {
    /// Package file(s) to install
    #[arg(required = true)]
    pub input: Vec<PathBuf>,

    /// Skip confirmation prompts
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Install as dependency
    #[arg(long)]
    pub asdeps: bool,

    /// Install as explicit
    #[arg(long)]
    pub asexplicit: bool,

    /// Pass additional flags to pacman
    #[arg(last = true)]
    pub pacman_args: Vec<String>,
}

/// Arguments for the config command
#[derive(Parser, Debug)]
pub struct ConfigArgs {
    /// Configuration subcommand
    #[command(subcommand)]
    pub command: ConfigCommands,
}

/// Configuration subcommands
#[derive(Subcommand, Debug)]
pub enum ConfigCommands {
    /// Show current configuration
    Show,
    /// Edit configuration file
    Edit,
    /// Reset configuration to defaults
    Reset,
    /// Set a configuration value
    Set {
        /// Configuration key
        key: String,
        /// Configuration value
        value: String,
    },
    /// Get a configuration value
    Get {
        /// Configuration key
        key: String,
    },
    /// Initialize configuration file
    Init {
        /// Force overwrite existing config
        #[arg(short, long)]
        force: bool,
    },
}

/// Arguments for the clean command
#[derive(Parser, Debug)]
pub struct CleanArgs {
    /// Clean package cache
    #[arg(long)]
    pub cache: bool,

    /// Clean temporary files
    #[arg(short, long)]
    pub temp: bool,

    /// Clean everything
    #[arg(short, long)]
    pub all: bool,

    /// Dry run - show what would be deleted
    #[arg(short, long)]
    pub dry_run: bool,
}

impl Cli {
    /// Parse command line arguments
    pub fn parse_args() -> Self {
        Self::parse()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_cli() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
