//! Command-line interface for rexeb

mod commands;
pub mod interactive;

pub use commands::*;

use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// Rexeb - A smarter, faster debtap alternative
///
/// Convert .deb and .rpm packages (or integrate AppImages) into Arch Linux
/// packages with intelligent dependency resolution and advanced features.
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

    /// Manage dependency mappings manually
    Map(MapArgs),

    /// Check if a package or converted package is available on AUR
    CheckAur(CheckAurArgs),

    /// Push a converted package to the AUR
    AurPush(AurPushArgs),

    /// List packages installed by rexeb (via watermark)
    ListInstalled(ListInstalledArgs),

    /// Manage installed rexeb packages (rename, fix icons)
    Manage(ManageArgs),

    /// Check for rexeb updates on GitHub
    SelfUpdate(SelfUpdateArgs),

    /// Print shell completions to stdout
    Completions(CompletionsArgs),

    /// Print (or write) the rexeb man page
    Manpage(ManpageArgs),

    /// Build a package from a sandbox manifest (internal: re-exec target)
    #[command(name = "__sandbox-build", hide = true)]
    SandboxBuild(SandboxBuildArgs),
}

/// Arguments for the convert command
#[derive(Parser, Debug, Clone)]
pub struct ConvertArgs {
    /// Input package file(s): .deb, .rpm, .AppImage (empty starts an interactive picker on a TTY)
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

    /// Interactive mode: picker, dependency review and confirmations
    #[arg(short = 'i', long)]
    pub interactive: bool,

    /// Show the conversion plan without writing anything
    #[arg(long)]
    pub dry_run: bool,

    /// Detach-sign the built package with GPG (uses the default key)
    #[arg(long)]
    pub sign: bool,

    /// Detach-sign with a specific GPG key id (implies --sign)
    #[arg(long)]
    pub sign_key: Option<String>,

    /// Treat package as 64-bit (experimental: only affects i386 packages)
    #[arg(short = 'P', long)]
    pub pseudo64: bool,

    /// Keep temporary files after conversion
    #[arg(long)]
    pub keep_temp: bool,

    /// Build inside a sandbox (see --sandbox-backend)
    #[arg(long)]
    pub sandbox: bool,

    /// Sandbox backend: bubblewrap isolates the build, nspawn only stages files
    #[arg(long, value_enum)]
    pub sandbox_backend: Option<SandboxBackendArg>,

    /// Custom package name override
    #[arg(long)]
    pub name: Option<String>,

    /// Custom version override
    #[arg(long)]
    pub version_override: Option<String>,

    /// Custom package release number
    #[arg(long)]
    pub release: Option<String>,

    /// Output format (overrides `conversion.default_format` from the config)
    #[arg(long, value_enum)]
    pub format: Option<OutputFormat>,
}

/// Sandbox backend for `--sandbox`
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum SandboxBackendArg {
    /// Bubblewrap when available, else nspawn staging
    Auto,
    /// Bubblewrap: the build runs fully isolated (rootless)
    Bwrap,
    /// systemd-nspawn: files are staged through a container (needs root)
    Nspawn,
}

/// Output format for converted packages
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, serde::Serialize, serde::Deserialize)]
pub enum OutputFormat {
    /// .pkg.tar.zst (default, recommended)
    #[value(name = "pkg.tar.zst", alias = "pkg-tar-zst", alias = "zst")]
    PkgTarZst,
    /// .pkg.tar.xz (legacy format)
    #[value(name = "pkg.tar.xz", alias = "pkg-tar-xz", alias = "xz")]
    PkgTarXz,
    /// .pkg.tar.gz (for compatibility)
    #[value(name = "pkg.tar.gz", alias = "pkg-tar-gz", alias = "gz")]
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

    /// Parse a format name from config (`pkg.tar.zst`, `pkg-tar-xz`, `gz`, ...)
    pub fn from_config_str(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "pkg.tar.zst" | "pkg-tar-zst" | "zst" => Some(Self::PkgTarZst),
            "pkg.tar.xz" | "pkg-tar-xz" | "xz" => Some(Self::PkgTarXz),
            "pkg.tar.gz" | "pkg-tar-gz" | "gz" => Some(Self::PkgTarGz),
            _ => None,
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

    /// Enlarge mappings by crawling local repo DBs and debtap mappings
    #[arg(long)]
    pub enlarge: bool,
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

    /// Build inside a systemd-nspawn sandbox
    #[arg(long)]
    pub sandbox: bool,

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
        /// Ask for the main settings instead of writing plain defaults
        #[arg(short = 'i', long)]
        interactive: bool,
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

/// Arguments for the map command (manual dependency mapping)
#[derive(Parser, Debug)]
pub struct MapArgs {
    /// Map subcommand
    #[command(subcommand)]
    pub command: MapCommands,
}

/// Map subcommands
#[derive(Subcommand, Debug)]
pub enum MapCommands {
    /// Add a manual mapping (missing values are prompted on a TTY)
    Add {
        /// Debian package name
        debian: Option<String>,
        /// Arch package name
        arch: Option<String>,
        /// Confidence (0.0 - 1.0)
        #[arg(long, default_value = "1.0")]
        confidence: f32,
    },
    /// Remove a mapping
    Remove {
        /// Debian package name
        debian: String,
    },
    /// List all mappings
    List {
        /// Output as JSON
        #[arg(long)]
        json: bool,
    },
    /// Import mappings from a JSON file
    Import {
        /// Path to JSON file
        file: PathBuf,
    },
    /// Export mappings to a JSON file
    Export {
        /// Output path
        file: PathBuf,
    },
}

/// Arguments for the check-aur command
#[derive(Parser, Debug)]
pub struct CheckAurArgs {
    /// Package file (.deb, .rpm, .AppImage) or package name to check
    pub package: Option<String>,
    /// Check all rexeb-installed packages
    #[arg(long)]
    pub installed: bool,
}

/// Arguments for the aur-push command
#[derive(Parser, Debug)]
pub struct AurPushArgs {
    /// Converted package directory (containing PKGBUILD) or package file
    pub input: PathBuf,
    /// AUR package base name (defaults to PKGBUILD pkgname)
    #[arg(long)]
    pub pkgbase: Option<String>,
    /// Show what would be done without pushing
    #[arg(long)]
    pub dry_run: bool,
    /// Force push even if package exists on AUR
    #[arg(long)]
    pub force: bool,
}

/// Arguments for list-installed command
#[derive(Parser, Debug)]
pub struct ListInstalledArgs {
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

/// Arguments for the manage command
#[derive(Parser, Debug)]
pub struct ManageArgs {
    /// Package name to manage
    pub package: String,
    /// Rename package to new name
    #[arg(long)]
    pub rename: Option<String>,
    /// Fix icon references for this package
    #[arg(long)]
    pub fix_icon: bool,
}

/// Arguments for self-update command
#[derive(Parser, Debug)]
pub struct SelfUpdateArgs {
    /// Only check, don't download
    #[arg(long)]
    pub check_only: bool,
    /// Force apply even if not needed
    #[arg(long)]
    pub force: bool,
}

/// Arguments for the completions command
#[derive(Parser, Debug)]
pub struct CompletionsArgs {
    /// Shell to generate completions for
    #[arg(value_enum)]
    pub shell: CompletionShell,
}

/// Shells supported by the `completions` command
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CompletionShell {
    /// Bourne-again shell
    Bash,
    /// Elvish shell
    Elvish,
    /// Friendly interactive shell
    Fish,
    /// PowerShell
    #[value(name = "powershell")]
    PowerShell,
    /// Z shell
    Zsh,
}

impl CompletionShell {
    /// Convert to the `clap_complete` generator
    pub fn to_generator(self) -> clap_complete::Shell {
        match self {
            Self::Bash => clap_complete::Shell::Bash,
            Self::Elvish => clap_complete::Shell::Elvish,
            Self::Fish => clap_complete::Shell::Fish,
            Self::PowerShell => clap_complete::Shell::PowerShell,
            Self::Zsh => clap_complete::Shell::Zsh,
        }
    }
}

/// Arguments for the manpage command
#[derive(Parser, Debug)]
pub struct ManpageArgs {
    /// Write to this file instead of stdout
    #[arg(short, long)]
    pub output: Option<PathBuf>,
}

/// Arguments for the hidden sandbox-build command
#[derive(Parser, Debug)]
pub struct SandboxBuildArgs {
    /// Path to the JSON build manifest
    #[arg(long)]
    pub manifest: PathBuf,
    /// Directory to write the built package into
    #[arg(long)]
    pub out_dir: PathBuf,
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
