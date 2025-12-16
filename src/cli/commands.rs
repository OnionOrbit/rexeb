//! Command execution handlers

use std::path::Path;

use crate::error::Result;

/// Execute the convert command
pub async fn execute_convert(args: &super::ConvertArgs) -> Result<()> {
    use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
    use rayon::prelude::*;

    let multi = MultiProgress::new();
    let style = ProgressStyle::with_template(
        "{prefix:.bold.dim} [{bar:40.cyan/blue}] {pos}/{len} {msg}"
    )
    .unwrap()
    .progress_chars("█▓▒░ ");

    let output_dir = args.output.clone().unwrap_or_else(|| std::env::current_dir().unwrap());
    
    // Process packages using tasks since we're async now
    let mut handles = Vec::new();
    
    for input_path in &args.input {
        let input_path = input_path.clone();
        let output_dir = output_dir.clone();
        let args_clone = args.clone();
        
        let pb = multi.add(ProgressBar::new(100));
        pb.set_style(style.clone());
        pb.set_prefix(format!("{}", input_path.file_name().unwrap_or_default().to_string_lossy()));
        
        handles.push(tokio::spawn(async move {
            convert_single_package(&input_path, &output_dir, &args_clone, pb).await
        }));
    }

    // Wait for all tasks
    for handle in handles {
        handle.await.map_err(|e| crate::error::RexebError::Other(e.to_string()))??;
    }

    Ok(())
}

/// Convert a single package
async fn convert_single_package(
    input: &Path,
    output_dir: &Path,
    args: &super::ConvertArgs,
    pb: indicatif::ProgressBar,
) -> Result<()> {
    use crate::converter::PackageConverter;
    use crate::parsers::deb::DebParser;

    pb.set_message("Parsing package...");
    pb.set_position(10);

    // Parse the deb package
    let parser = DebParser::new(input)?;
    let mut metadata = parser.parse()?;

    pb.set_position(30);
    pb.set_message("Resolving dependencies...");

    // Apply overrides
    if let Some(ref name) = args.name {
        metadata.arch_name = Some(name.clone());
    }
    if let Some(ref version) = args.version_override {
        metadata.version = version.clone();
    }
    if let Some(ref release) = args.release {
        metadata.release = release.clone();
    }

    // Normalize version
    metadata.normalize_version();

    pb.set_position(40);

    // Resolve dependencies if not skipped
    if !args.skip_deps {
        let resolver = crate::resolver::DependencyResolver::new()?;
        resolver.resolve(&mut metadata).await?;
    }

    pb.set_position(60);
    pb.set_message("Building package...");

    // Create output package
    if args.pkgbuild {
        // Generate PKGBUILD
        let pkgbuild_path = output_dir.join("PKGBUILD");
        std::fs::write(&pkgbuild_path, metadata.to_pkgbuild())?;
        pb.set_position(100);
        pb.finish_with_message(format!("Created {}", pkgbuild_path.display()));
    } else {
        // Build binary package
        let converter = PackageConverter::new(metadata, parser.extract_dir())?;
        let output_path = converter.build(output_dir, args.format)?;
        pb.set_position(100);
        pb.finish_with_message(format!("Created {}", output_path.display()));
    }

    Ok(())
}

/// Execute the update command
pub async fn execute_update(args: &super::UpdateArgs) -> Result<()> {
    use crate::resolver::database::PackageDatabase;
    use indicatif::{ProgressBar, ProgressStyle};

    let style = ProgressStyle::with_template(
        "{spinner:.green} [{bar:40.cyan/blue}] {msg}"
    )
    .unwrap();

    let pb = ProgressBar::new_spinner();
    pb.set_style(style);

    let update_all = args.all || (!args.virtual_packages && !args.mappings && !args.aur);

    let db = PackageDatabase::new()?;

    if update_all || args.mappings {
        pb.set_message("Updating package mappings...");
        db.update_mappings(args.force).await?;
    }

    if update_all || args.virtual_packages {
        pb.set_message("Updating virtual packages database...");
        db.update_virtual_packages(args.force).await?;
    }

    if update_all || args.aur {
        pb.set_message("Updating AUR cache...");
        db.update_aur_cache(args.force).await?;
    }

    pb.finish_with_message("Database updated successfully");
    Ok(())
}

/// Execute the info command
pub async fn execute_info(args: &super::InfoArgs) -> Result<()> {
    use crate::parsers::deb::DebParser;

    let parser = DebParser::new(&args.package)?;
    let metadata = parser.parse()?;

    match args.format {
        super::InfoFormat::Pretty => {
            println!("Package Information");
            println!("═══════════════════════════════════════");
            println!("Name:        {}", metadata.name);
            println!("Version:     {}", metadata.full_version());
            println!("Architecture: {}", metadata.arch);
            println!("Description: {}", metadata.description);
            
            if let Some(ref url) = metadata.url {
                println!("URL:         {}", url);
            }
            
            println!("License:     {}", metadata.license.to_pkgbuild());
            println!("Size:        {} bytes", metadata.installed_size);
            
            if let Some(ref maintainer) = metadata.maintainer {
                println!("Maintainer:  {}", maintainer);
            }

            if args.extended {
                println!("\nDependencies:");
                for dep in metadata.get_deps(crate::models::DependencyType::Depends) {
                    println!("  - {}", dep);
                }

                println!("\nFiles: {} total", metadata.files.len());
                for file in metadata.files.iter().take(10) {
                    println!("  {}", file.display());
                }
                if metadata.files.len() > 10 {
                    println!("  ... and {} more", metadata.files.len() - 10);
                }
            }
        }
        super::InfoFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&metadata)?);
        }
        super::InfoFormat::Toml => {
            println!("{}", toml::to_string_pretty(&metadata).map_err(|e| crate::error::RexebError::Other(e.to_string()))?);
        }
    }

    Ok(())
}

/// Execute the search command
pub async fn execute_search(args: &super::SearchArgs) -> Result<()> {
    use crate::resolver::database::PackageDatabase;
    use console::style;

    let db = PackageDatabase::new()?;
    
    let search_arch = args.arch || (!args.arch && !args.aur);
    let search_aur = args.aur || (!args.arch && !args.aur);

    let mut results = Vec::new();

    if search_arch {
        let arch_results = db.search_arch(&args.query, args.fuzzy, args.limit).await?;
        results.extend(arch_results.into_iter().map(|r| ("arch", r)));
    }

    if search_aur {
        let aur_results = db.search_aur(&args.query, args.fuzzy, args.limit).await?;
        results.extend(aur_results.into_iter().map(|r| ("aur", r)));
    }

    if results.is_empty() {
        println!("No packages found matching '{}'", args.query);
    } else {
        println!("Search Results for '{}'\n", style(&args.query).cyan());
        
        for (source, result) in results.iter().take(args.limit) {
            let source_badge = match *source {
                "arch" => style("[arch]").green(),
                "aur" => style("[aur]").yellow(),
                _ => style("[?]").dim(),
            };
            
            println!("{} {} - {}", source_badge, style(&result.name).bold(), result.description);
        }
    }

    Ok(())
}

/// Execute the analyze command
pub async fn execute_analyze(args: &super::AnalyzeArgs) -> Result<()> {
    use crate::parsers::deb::DebParser;
    use crate::analyzer::PackageAnalyzer;
    use console::style;

    let parser = DebParser::new(&args.input)?;
    let metadata = parser.parse()?;

    let analyzer = PackageAnalyzer::new(&metadata, parser.extract_dir())?;
    let report = analyzer.analyze(args.conflicts, args.verify)?;

    match args.format {
        super::InfoFormat::Pretty => {
            println!("{}", style("Package Analysis Report").bold().underlined());
            println!();
            
            // Summary
            println!("{}", style("Summary").bold());
            println!("  Package: {} {}", metadata.name, metadata.full_version());
            println!("  Architecture: {}", metadata.arch);
            println!("  Files: {}", metadata.files.len());
            println!("  Installed Size: {} KB", metadata.installed_size / 1024);
            println!();

            // Warnings
            if !report.warnings.is_empty() {
                println!("{}", style("⚠ Warnings").yellow().bold());
                for warning in &report.warnings {
                    println!("  • {}", warning);
                }
                println!();
            }

            // Errors
            if !report.errors.is_empty() {
                println!("{}", style("✗ Errors").red().bold());
                for error in &report.errors {
                    println!("  • {}", error);
                }
                println!();
            }

            // Dependency Analysis
            println!("{}", style("Dependency Analysis").bold());
            println!("  Total: {}", report.dependency_count);
            println!("  Mapped: {} ({:.1}%)", 
                report.mapped_count, 
                (report.mapped_count as f32 / report.dependency_count.max(1) as f32) * 100.0
            );
            println!("  Unmapped: {}", report.unmapped_deps.len());
            
            if !report.unmapped_deps.is_empty() {
                println!("\n  Unmapped dependencies:");
                for dep in &report.unmapped_deps {
                    println!("    - {}", dep);
                }
            }

            // Conflicts
            if args.conflicts && !report.conflicts.is_empty() {
                println!("\n{}", style("! Conflicts").red().bold());
                for conflict in &report.conflicts {
                    println!("  • {}", conflict);
                }
            }

            // File analysis
            if args.verify {
                println!("\n{}", style("File Verification").bold());
                println!("  Verified: {}", report.verified_files);
                println!("  Failed: {}", report.failed_files);
            }
        }
        super::InfoFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        super::InfoFormat::Toml => {
            println!("{}", toml::to_string_pretty(&report).map_err(|e| crate::error::RexebError::Other(e.to_string()))?);
        }
    }

    Ok(())
}

/// Execute the install command
pub async fn execute_install(args: &super::InstallArgs) -> Result<()> {
    use std::process::Command;
    use tempfile::TempDir;

    // Convert packages first
    let temp_dir = TempDir::new()?;
    let convert_args = super::ConvertArgs {
        input: args.input.clone(),
        output: Some(temp_dir.path().to_path_buf()),
        skip_deps: false,
        force: false,
        pkgbuild: false,
        yes: args.yes,
        pseudo64: false,
        keep_temp: false,
        name: None,
        version_override: None,
        release: None,
        format: super::OutputFormat::PkgTarZst,
    };

    execute_convert(&convert_args).await?;

    // Find converted packages
    let packages: Vec<_> = std::fs::read_dir(temp_dir.path())?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "zst"))
        .map(|e| e.path())
        .collect();

    if packages.is_empty() {
        return Err(crate::error::RexebError::PackageBuild("No packages were created".into()));
    }

    // Build pacman command
    let mut cmd = Command::new("sudo");
    cmd.arg("pacman").arg("-U");

    if args.yes {
        cmd.arg("--noconfirm");
    }
    if args.asdeps {
        cmd.arg("--asdeps");
    }
    if args.asexplicit {
        cmd.arg("--asexplicit");
    }

    for arg in &args.pacman_args {
        cmd.arg(arg);
    }

    for pkg in &packages {
        cmd.arg(pkg);
    }

    // Execute pacman
    let status = cmd.status()?;

    if !status.success() {
        return Err(crate::error::RexebError::Other(
            format!("pacman exited with status: {}", status)
        ));
    }

    Ok(())
}

/// Execute the config command
pub async fn execute_config(args: &super::ConfigArgs) -> Result<()> {
    use crate::config::Config;

    match &args.command {
        super::ConfigCommands::Show => {
            let config = Config::load()?;
            println!("{}", toml::to_string_pretty(&config).map_err(|e| crate::error::RexebError::Other(e.to_string()))?);
        }
        super::ConfigCommands::Edit => {
            let config_path = Config::config_path()?;
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());
            std::process::Command::new(editor)
                .arg(&config_path)
                .status()?;
        }
        super::ConfigCommands::Reset => {
            Config::reset()?;
            println!("Configuration reset to defaults");
        }
        super::ConfigCommands::Set { key, value } => {
            let mut config = Config::load()?;
            config.set(key, value)?;
            config.save()?;
            println!("Set {} = {}", key, value);
        }
        super::ConfigCommands::Get { key } => {
            let config = Config::load()?;
            if let Some(value) = config.get(key) {
                println!("{}", value);
            } else {
                println!("Key '{}' not found", key);
            }
        }
        super::ConfigCommands::Init { force } => {
            Config::init(*force)?;
            println!("Configuration initialized");
        }
    }

    Ok(())
}

/// Execute the clean command
pub async fn execute_clean(args: &super::CleanArgs) -> Result<()> {
    use crate::config::Config;

    let config = Config::load()?;
    let clean_all = args.all || (!args.cache && !args.temp);

    let mut cleaned = Vec::new();

    if clean_all || args.cache {
        let cache_dir = config.cache_dir();
        if cache_dir.exists() {
            if args.dry_run {
                println!("Would remove: {}", cache_dir.display());
            } else {
                std::fs::remove_dir_all(&cache_dir)?;
                cleaned.push(cache_dir);
            }
        }
    }

    if clean_all || args.temp {
        let temp_dir = std::env::temp_dir().join("rexeb");
        if temp_dir.exists() {
            if args.dry_run {
                println!("Would remove: {}", temp_dir.display());
            } else {
                std::fs::remove_dir_all(&temp_dir)?;
                cleaned.push(temp_dir);
            }
        }
    }

    if !args.dry_run {
        if cleaned.is_empty() {
            println!("Nothing to clean");
        } else {
            println!("Cleaned {} directories", cleaned.len());
        }
    }

    Ok(())
}
